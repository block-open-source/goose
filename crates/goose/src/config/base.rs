use crate::config::paths::Paths;
use crate::config::GooseMode;
use crate::providers::private_file::{private_file_target_path, write_private_file};
use fs2::FileExt;
use goose_providers::thinking::ThinkingEffort;
#[cfg(feature = "system-keyring")]
use keyring::Entry;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_yaml::Mapping;
use std::collections::HashMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

fn write_secrets_file(path: &Path, content: &str) -> std::io::Result<()> {
    write_private_file(path, content)
}

fn secrets_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

#[cfg(feature = "system-keyring")]
const KEYRING_SERVICE: &str = "goose";
#[cfg(feature = "system-keyring")]
const KEYRING_USERNAME: &str = "secrets";
pub const CONFIG_YAML_NAME: &str = "config.yaml";

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Configuration value not found: {0}")]
    NotFound(String),
    #[error("Failed to deserialize value: {0}")]
    DeserializeError(String),
    #[error("Failed to read config file: {0}")]
    FileError(#[from] std::io::Error),
    #[error("Failed to create config directory: {0}")]
    DirectoryError(String),
    #[error("Failed to access keyring: {0}")]
    KeyringError(String),
    #[error("Failed to lock config file: {0}")]
    LockError(String),
    #[error("Secret stored using file-based fallback")]
    FallbackToFileStorage,
}

impl From<serde_json::Error> for ConfigError {
    fn from(err: serde_json::Error) -> Self {
        ConfigError::DeserializeError(err.to_string())
    }
}

impl From<serde_yaml::Error> for ConfigError {
    fn from(err: serde_yaml::Error) -> Self {
        ConfigError::DeserializeError(err.to_string())
    }
}

#[cfg(feature = "system-keyring")]
impl From<keyring::Error> for ConfigError {
    fn from(err: keyring::Error) -> Self {
        ConfigError::KeyringError(err.to_string())
    }
}

/// Configuration management for goose.
///
/// This module provides a flexible configuration system that supports:
/// - Dynamic configuration keys
/// - Multiple value types through serde deserialization
/// - Environment variable overrides
/// - YAML-based configuration file storage
/// - Hot reloading of configuration changes
/// - Secure secret storage in system keyring
///
/// Configuration values are loaded with the following precedence:
/// 1. Environment variables (exact key match)
/// 2. Configuration file (~/.config/goose/config.yaml by default)
///
/// Secrets are loaded with the following precedence:
/// 1. Environment variables (exact key match)
/// 2. System keyring (which can be disabled with GOOSE_DISABLE_KEYRING)
/// 3. If the keyring is disabled, secrets are stored in a secrets file
///    (~/.config/goose/secrets.yaml by default)
///
/// # Examples
///
/// ```no_run
/// use goose::config::Config;
/// use serde::Deserialize;
///
/// // Get a string value
/// let config = Config::global();
/// let api_key: String = config.get_param("OPENAI_API_KEY").unwrap();
///
/// // Get a complex type
/// #[derive(Deserialize)]
/// struct ServerConfig {
///     host: String,
///     port: u16,
/// }
///
/// let server_config: ServerConfig = config.get_param("server").unwrap();
/// ```
///
/// # Naming Convention
/// we recommend snake_case for keys, and will convert to UPPERCASE when
/// checking for environment overrides. e.g. openai_api_key will check for an
/// environment variable OPENAI_API_KEY
///
/// For goose-specific configuration, consider prefixing with "goose_" to avoid conflicts.
pub struct Config {
    /// Ordered list of config files to load and merge.
    /// Later entries take precedence over earlier ones.
    /// The last entry is where changes will be written.
    config_paths: Vec<PathBuf>,
    secrets: SecretStorage,
    guard: Mutex<()>,
    secrets_cache: Arc<Mutex<Option<HashMap<String, Value>>>>,
}

enum SecretStorage {
    #[cfg(feature = "system-keyring")]
    Keyring {
        service: String,
    },
    File {
        path: PathBuf,
    },
}

enum SecretMutation<T> {
    Write(T),
    Unchanged(T),
}

pub(crate) enum SecretUpdate<V, R> {
    Write(V, R),
    Unchanged(R),
}

// Global instance
static GLOBAL_CONFIG: OnceCell<Config> = OnceCell::new();

#[cfg(test)]
pub(crate) const TEST_SYSTEM_CONFIG_PATH_ENV: &str = "GOOSE_TEST_SYSTEM_CONFIG_PATH";

fn system_config_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = env::var_os(TEST_SYSTEM_CONFIG_PATH_ENV) {
        return path.into();
    }

    #[cfg(unix)]
    {
        PathBuf::from("/etc/goose/config.yaml")
    }
    #[cfg(windows)]
    {
        env::var("PROGRAMDATA")
            .map(|d| PathBuf::from(d).join("goose").join("config.yaml"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\ProgramData\goose\config.yaml"))
    }
}

fn additional_config_paths_from_env() -> Vec<PathBuf> {
    env::var_os("GOOSE_ADDITIONAL_CONFIG_FILES")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default()
}

fn metadata_is_symlink_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

impl Default for Config {
    fn default() -> Self {
        let config_dir = Paths::config_dir();
        let user_config_path = config_dir.join(CONFIG_YAML_NAME);

        let mut config_paths = vec![system_config_path()];
        config_paths.extend(additional_config_paths_from_env());
        config_paths.push(user_config_path.clone());

        let no_secrets_config = Self {
            config_paths: config_paths.clone(),
            secrets: SecretStorage::File {
                path: Default::default(),
            },
            guard: Mutex::new(()),
            secrets_cache: Arc::new(Mutex::new(None)),
        };

        let keyring_disabled = env::var("GOOSE_DISABLE_KEYRING").is_ok()
            || no_secrets_config
                .get_param::<serde_yaml::Value>("GOOSE_DISABLE_KEYRING")
                .is_ok_and(|v| keyring_disabled_value(&v));
        let secrets = secret_storage(&config_dir, keyring_disabled, default_keyring_service());
        Self {
            config_paths,
            secrets,
            guard: Mutex::new(()),
            secrets_cache: Arc::new(Mutex::new(None)),
        }
    }
}

pub trait ConfigValue {
    const KEY: &'static str;
    const DEFAULT: &'static str;
}

macro_rules! config_value {
    ($key:ident, $type:ty) => {
        impl Config {
            pastey::paste! {
                pub fn [<get_ $key:lower>](&self) -> Result<$type, ConfigError> {
                    self.get_param(stringify!($key))
                }
            }
            pastey::paste! {
                pub fn [<set_ $key:lower>](&self, v: impl Into<$type>) -> Result<(), ConfigError> {
                    self.set_param(stringify!($key), &v.into())
                }
            }
        }
    };

    ($key:ident, $inner:ty, $default:expr) => {
        pastey::paste! {
            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[serde(transparent)]
            pub struct [<$key:camel>]($inner);

            impl ConfigValue for [<$key:camel>] {
                const KEY: &'static str = stringify!($key);
                const DEFAULT: &'static str = $default;
            }

            impl Default for [<$key:camel>] {
                fn default() -> Self {
                    [<$key:camel>]($default.into())
                }
            }

            impl std::ops::Deref for [<$key:camel>] {
                type Target = $inner;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl std::ops::DerefMut for [<$key:camel>] {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }

            impl std::fmt::Display for [<$key:camel>] {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{:?}", self.0)
                }
            }

            impl From<$inner> for [<$key:camel>] {
                fn from(value: $inner) -> Self {
                    [<$key:camel>](value)
                }
            }

            impl From<[<$key:camel>]> for $inner {
                fn from(value: [<$key:camel>]) -> $inner {
                    value.0
                }
            }

            config_value!($key, [<$key:camel>]);
        }
    };
}

fn parse_yaml_content(content: &str) -> Result<Mapping, ConfigError> {
    serde_yaml::from_str(content).map_err(|e| e.into())
}

fn keyring_disabled_value(value: &serde_yaml::Value) -> bool {
    value.as_bool().unwrap_or(false) || value.as_str().is_some_and(|s| s == "true" || s == "1")
}

const EXTENSIONS_KEY: &str = "extensions";
const PROVIDERS_KEY: &str = "providers";

pub fn merge_config_values(base: &mut Mapping, overlay: Mapping) {
    let extensions_key = serde_yaml::Value::String(EXTENSIONS_KEY.to_string());
    let providers_key = serde_yaml::Value::String(PROVIDERS_KEY.to_string());

    for (key, overlay_value) in overlay {
        if key == extensions_key {
            let base_ext = base
                .entry(key.clone())
                .or_insert_with(|| serde_yaml::Value::Mapping(Mapping::new()));
            if let (Some(base_map), Some(overlay_map)) =
                (base_ext.as_mapping_mut(), overlay_value.as_mapping())
            {
                merge_nested_entries(base_map, overlay_map);
            } else {
                base.insert(key, overlay_value);
            }
        } else if key == providers_key {
            let base_prov = base
                .entry(key.clone())
                .or_insert_with(|| serde_yaml::Value::Mapping(Mapping::new()));
            if let (Some(base_map), Some(overlay_map)) =
                (base_prov.as_mapping_mut(), overlay_value.as_mapping())
            {
                merge_nested_entries(base_map, overlay_map);
            } else {
                base.insert(key, overlay_value);
            }
        } else {
            base.insert(key, overlay_value);
        }
    }
}

fn merge_nested_entries(base: &mut Mapping, overlay: &Mapping) {
    for (ext_key, overlay_ext) in overlay {
        match base.get_mut(ext_key) {
            Some(base_ext) => {
                if let (Some(base_map), Some(overlay_map)) =
                    (base_ext.as_mapping_mut(), overlay_ext.as_mapping())
                {
                    for (field_key, field_value) in overlay_map {
                        base_map.insert(field_key.clone(), field_value.clone());
                    }
                } else {
                    *base_ext = overlay_ext.clone();
                }
            }
            None => {
                base.insert(ext_key.clone(), overlay_ext.clone());
            }
        }
    }
}

/// Read the GOOSE_DISABLE_KEYRING flag from the config file.
///
/// Called before Config is fully initialised, so we do a minimal raw read
/// rather than going through `get_param`.  All errors are treated as `false`
/// (keyring stays enabled) so a missing/malformed file is never fatal here.
fn keyring_disabled_in_config(config_path: &Path) -> bool {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| parse_yaml_content(&s).ok())
        .and_then(|m| m.get("GOOSE_DISABLE_KEYRING").map(keyring_disabled_value))
        .unwrap_or(false)
}

#[cfg(feature = "system-keyring")]
fn default_keyring_service() -> &'static str {
    KEYRING_SERVICE
}

#[cfg(not(feature = "system-keyring"))]
fn default_keyring_service() -> &'static str {
    ""
}

fn secrets_file_path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("secrets.yaml")
}

#[cfg(feature = "system-keyring")]
fn secret_storage(config_dir: &Path, keyring_disabled: bool, service: &str) -> SecretStorage {
    if keyring_disabled {
        SecretStorage::File {
            path: secrets_file_path_in(config_dir),
        }
    } else {
        SecretStorage::Keyring {
            service: service.to_string(),
        }
    }
}

#[cfg(not(feature = "system-keyring"))]
fn secret_storage(config_dir: &Path, _keyring_disabled: bool, _service: &str) -> SecretStorage {
    SecretStorage::File {
        path: secrets_file_path_in(config_dir),
    }
}

impl Config {
    /// Get the global configuration instance.
    ///
    /// This will initialize the configuration with the default path (~/.config/goose/config.yaml)
    /// if it hasn't been initialized yet.
    pub fn global() -> &'static Config {
        GLOBAL_CONFIG.get_or_init(Config::default)
    }

    /// Create a new configuration instance with custom paths
    ///
    /// This is primarily useful for testing or for applications that need
    /// to manage multiple configuration files.
    pub fn new<P: AsRef<Path>>(config_path: P, service: &str) -> Result<Self, ConfigError> {
        let config_path = config_path.as_ref().to_path_buf();
        let keyring_disabled =
            env::var("GOOSE_DISABLE_KEYRING").is_ok() || keyring_disabled_in_config(&config_path);
        let config_dir = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(Paths::config_dir);
        let secrets = secret_storage(&config_dir, keyring_disabled, service);
        Ok(Config {
            config_paths: vec![config_path],
            secrets,
            guard: Mutex::new(()),
            secrets_cache: Arc::new(Mutex::new(None)),
        })
    }

    /// Create a new configuration instance with custom paths
    ///
    /// This is primarily useful for testing or for applications that need
    /// to manage multiple configuration files.
    pub fn new_with_file_secrets<P1: AsRef<Path>, P2: AsRef<Path>>(
        config_path: P1,
        secrets_path: P2,
    ) -> Result<Self, ConfigError> {
        Ok(Config {
            config_paths: vec![config_path.as_ref().to_path_buf()],
            secrets: SecretStorage::File {
                path: secrets_path.as_ref().to_path_buf(),
            },
            guard: Mutex::new(()),
            secrets_cache: Arc::new(Mutex::new(None)),
        })
    }

    pub fn new_with_config_paths<P1: AsRef<Path>>(
        config_paths: Vec<PathBuf>,
        secrets_path: P1,
    ) -> Result<Self, ConfigError> {
        Ok(Config {
            config_paths,
            secrets: SecretStorage::File {
                path: secrets_path.as_ref().to_path_buf(),
            },
            guard: Mutex::new(()),
            secrets_cache: Arc::new(Mutex::new(None)),
        })
    }

    fn write_path(&self) -> &PathBuf {
        self.config_paths
            .last()
            .expect("config_paths must not be empty")
    }

    pub fn exists(&self) -> bool {
        self.config_paths.iter().any(|p| p.exists())
    }

    pub fn clear(&self) -> Result<(), ConfigError> {
        Ok(std::fs::remove_file(self.write_path())?)
    }

    pub fn path(&self) -> String {
        self.write_path().to_string_lossy().to_string()
    }

    /// Load only the writable config file for read-modify-write operations.
    /// Returns an empty mapping if the file doesn't exist or can't be parsed.
    fn load_write_config(&self) -> Result<Mapping, ConfigError> {
        if !self.write_path().exists() {
            return Ok(Mapping::new());
        }
        let content = std::fs::read_to_string(self.write_path())?;
        let mut values = parse_yaml_content(&content).unwrap_or_else(|e| {
            tracing::warn!(
                "Config file {:?} is corrupt: {}. Starting fresh.",
                self.write_path(),
                e
            );
            Mapping::new()
        });

        if crate::config::migrations::run_migrations(&mut values) {
            if let Err(e) = self.save_values(&values) {
                tracing::warn!("Failed to save migrated config: {}", e);
            }
        }

        Ok(values)
    }

    fn load(&self) -> Result<Mapping, ConfigError> {
        let mut merged = Mapping::new();

        for path in &self.config_paths {
            if !path.exists() {
                continue;
            }
            match std::fs::read_to_string(path)
                .map_err(ConfigError::from)
                .and_then(|content| parse_yaml_content(&content))
            {
                Ok(layer) => {
                    tracing::debug!("Loading config from: {:?}", path);
                    merge_config_values(&mut merged, layer);
                }
                Err(e) => {
                    tracing::warn!("Failed to load config {:?}: {}. Skipping.", path, e);
                }
            }
        }

        crate::config::migrations::run_read_migrations(&mut merged);

        Ok(merged)
    }

    fn load_strict(&self) -> Result<Mapping, ConfigError> {
        let mut merged = Mapping::new();

        for path in &self.config_paths {
            match std::fs::symlink_metadata(path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    for ancestor in path.ancestors().skip(1) {
                        match std::fs::symlink_metadata(ancestor) {
                            Ok(metadata) if metadata_is_symlink_or_reparse_point(&metadata) => {
                                std::fs::metadata(ancestor)?;
                            }
                            Ok(_) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let content = std::fs::read_to_string(path)?;
            let layer = parse_yaml_content(&content)?;
            merge_config_values(&mut merged, layer);
        }

        crate::config::migrations::run_read_migrations(&mut merged);

        Ok(merged)
    }

    pub fn all_values(&self) -> Result<HashMap<String, Value>, ConfigError> {
        let config_values = self.load()?;
        let mut map = HashMap::from_iter(config_values.into_iter().filter_map(|(k, v)| {
            k.as_str()
                .map(|k| k.to_string())
                .zip(serde_json::to_value(v).ok())
        }));

        if let Ok(provider) = self.get_goose_provider() {
            map.insert("GOOSE_PROVIDER".to_string(), Value::String(provider));
        }
        if let Ok(model) = self.get_goose_model() {
            map.insert("GOOSE_MODEL".to_string(), Value::String(model));
        }

        Ok(map)
    }

    fn config_write_target_path(&self) -> Result<PathBuf, ConfigError> {
        let mut path = self.write_path().clone();

        // Follow symlinks so we update the target file without replacing the link itself.
        const MAX_SYMLINK_HOPS: usize = 1;
        let mut hops = 0usize;
        loop {
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    if hops >= MAX_SYMLINK_HOPS {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "Too many symlink levels (or a cycle) while resolving config path: {:?}",
                                self.write_path()
                            ),
                        )
                        .into());
                    }
                    hops += 1;

                    let link = std::fs::read_link(&path)?;
                    path = if link.is_absolute() {
                        link
                    } else {
                        path.parent().unwrap_or_else(|| Path::new(".")).join(link)
                    };
                }
                Ok(_) => return Ok(path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(path),
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn save_values(&self, values: &Mapping) -> Result<(), ConfigError> {
        let target_path = self.config_write_target_path()?;

        // Convert to YAML for storage
        let yaml_value = serde_yaml::to_string(values)?;

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError::DirectoryError(e.to_string()))?;
        }

        // Write to a temporary file first for atomic operation
        let temp_path = target_path.with_extension("tmp");

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)?;

            // Acquire an exclusive lock
            file.lock_exclusive()
                .map_err(|e| ConfigError::LockError(e.to_string()))?;

            // Write the contents using the same file handle
            file.write_all(yaml_value.as_bytes())?;
            file.sync_all()?;

            // Unlock is handled automatically when file is dropped
        }

        // Atomically replace the original file
        std::fs::rename(&temp_path, &target_path)?;

        Ok(())
    }

    pub fn initialize_if_empty(&self, values: Mapping) -> Result<(), ConfigError> {
        let _guard = self.guard.lock().unwrap();
        if !self.exists() {
            self.save_values(&values)
        } else {
            Ok(())
        }
    }

    pub fn all_secrets(&self) -> Result<HashMap<String, Value>, ConfigError> {
        let mut cache = self.secrets_cache.lock().unwrap();

        let values = if let Some(ref cached_secrets) = *cache {
            cached_secrets.clone()
        } else {
            tracing::debug!("secrets cache miss, fetching from storage");
            let loaded = self.load_secrets_from_storage()?;

            *cache = Some(loaded.clone());
            loaded
        };

        Ok(values)
    }

    /// Parse an environment variable value into a JSON Value.
    ///
    /// This function tries to intelligently parse environment variable values:
    /// 1. First attempts JSON parsing (for structured data)
    /// 2. If that fails, tries primitive type parsing for common cases
    /// 3. Falls back to string if nothing else works
    fn parse_env_value(val: &str) -> Result<Value, ConfigError> {
        // First try JSON parsing - this handles quoted strings, objects, arrays, etc.
        if let Ok(json_value) = serde_json::from_str(val) {
            return Ok(json_value);
        }

        let trimmed = val.trim();

        match trimmed.to_lowercase().as_str() {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            _ => {}
        }

        if let Ok(int_val) = trimmed.parse::<i64>() {
            return Ok(Value::Number(int_val.into()));
        }

        if let Ok(float_val) = trimmed.parse::<f64>() {
            if let Some(num) = serde_json::Number::from_f64(float_val) {
                return Ok(Value::Number(num));
            }
        }

        Ok(Value::String(val.to_string()))
    }

    // check all possible places for a parameter
    pub fn get(&self, key: &str, is_secret: bool) -> Result<Value, ConfigError> {
        if is_secret {
            self.get_secret(key)
        } else {
            self.get_param(key)
        }
    }

    // save a parameter in the appropriate location based on if it's secret or not
    pub fn set<V>(&self, key: &str, value: &V, is_secret: bool) -> Result<(), ConfigError>
    where
        V: Serialize,
    {
        if is_secret {
            self.set_secret(key, value)
        } else {
            self.set_param(key, value)
        }
    }

    /// Get a configuration value (non-secret).
    ///
    /// This will attempt to get the value from (in order):
    /// 1. Environment variable with the uppercase key name
    /// 2. Merged config from all config paths (system → user → local)
    ///
    /// The value will be deserialized into the requested type. This works with
    /// both simple types (String, i32, etc.) and complex types that implement
    /// serde::Deserialize.
    ///
    /// # Errors
    ///
    /// Returns a ConfigError if:
    /// - The key doesn't exist in any of the above sources
    /// - The value cannot be deserialized into the requested type
    /// - There is an error reading the config file
    pub fn get_param<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<T, ConfigError> {
        let env_key = key.to_uppercase();
        if let Ok(val) = env::var(&env_key) {
            let value = Self::parse_env_value(&val)?;
            return Ok(serde_json::from_value(value)?);
        }

        let values = self.load()?;
        let value = values
            .get(key)
            .ok_or_else(|| ConfigError::NotFound(key.to_string()))?;

        match serde_yaml::from_value(value.clone()) {
            Ok(value) => Ok(value),
            Err(yaml_err) => {
                let Some(string_value) = value.as_str() else {
                    return Err(yaml_err.into());
                };
                let parsed = Self::parse_env_value(string_value)?;
                serde_json::from_value(parsed).map_err(|_| yaml_err.into())
            }
        }
    }

    pub(crate) fn get_param_source_values<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Vec<T>, ConfigError> {
        let mut source_values = Vec::new();
        let env_key = key.to_uppercase();
        if let Some(value) = env::var_os(&env_key) {
            let value = value.into_string().map_err(|_| {
                ConfigError::DeserializeError(format!(
                    "environment variable {env_key} is not valid UTF-8"
                ))
            })?;
            source_values.push(serde_json::from_value(Self::parse_env_value(&value)?)?);
        }

        for path in &self.config_paths {
            let content = match std::fs::read_to_string(path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let mut values = parse_yaml_content(&content)?;
            crate::config::migrations::run_read_migrations(&mut values);
            let Some(value) = values.get(key) else {
                continue;
            };
            source_values.push(serde_yaml::from_value(value.clone())?);
        }

        Ok(source_values)
    }

    /// Read-modify-write a configuration value atomically through the write path.
    pub fn update_param<T, V, F>(&self, key: &str, f: F) -> Result<(), ConfigError>
    where
        T: for<'de> Deserialize<'de> + Default,
        V: Serialize,
        F: FnOnce(T) -> V,
    {
        let _guard = self.guard.lock().unwrap();
        let mut values = self.load_write_config()?;
        let current: T = values
            .get(key)
            .and_then(|v| serde_yaml::from_value(v.clone()).ok())
            .unwrap_or_default();
        let updated = f(current);
        values.insert(serde_yaml::to_value(key)?, serde_yaml::to_value(updated)?);
        self.save_values(&values)
    }

    /// Set a configuration value in the config file (non-secret).
    ///
    /// This will immediately write the value to the config file. The value
    /// can be any type that can be serialized to JSON/YAML.
    ///
    /// Note that this does not affect environment variables - those can only
    /// be set through the system environment.
    ///
    /// # Errors
    ///
    /// Returns a ConfigError if:
    /// - There is an error reading or writing the config file
    /// - There is an error serializing the value
    pub fn set_param<V: Serialize>(&self, key: &str, value: V) -> Result<(), ConfigError> {
        let _guard = self.guard.lock().unwrap();
        let mut values = self.load_write_config()?;
        values.insert(serde_yaml::to_value(key)?, serde_yaml::to_value(value)?);
        self.save_values(&values)
    }

    /// Set multiple configuration values in the config file with one read and one write.
    pub fn set_param_values(&self, updates: &[(String, Value)]) -> Result<(), ConfigError> {
        if updates.is_empty() {
            return Ok(());
        }

        let _guard = self.guard.lock().unwrap();
        let mut values = self.load_write_config()?;
        for (key, value) in updates {
            values.insert(serde_yaml::to_value(key)?, serde_yaml::to_value(value)?);
        }
        self.save_values(&values)
    }

    /// Delete a configuration value in the config file.
    ///
    /// This will immediately write the value to the config file. The value
    /// can be any type that can be serialized to JSON/YAML.
    ///
    /// Note that this does not affect environment variables - those can only
    /// be set through the system environment.
    ///
    /// # Errors
    ///
    /// Returns a ConfigError if:
    /// - There is an error reading or writing the config file
    /// - There is an error serializing the value
    pub fn delete(&self, key: &str) -> Result<(), ConfigError> {
        // Lock before reading to prevent race condition.
        let _guard = self.guard.lock().unwrap();

        let mut values = self.load_write_config()?;
        values.shift_remove(key);

        self.save_values(&values)
    }

    /// Get a secret value.
    ///
    /// This will attempt to get the value from:
    /// 1. Environment variable with the exact key name
    /// 2. System keyring
    ///
    /// The value will be deserialized into the requested type. This works with
    /// both simple types (String, i32, etc.) and complex types that implement
    /// serde::Deserialize.
    ///
    /// # Errors
    ///
    /// Returns a ConfigError if:
    /// - The key doesn't exist in either environment or keyring
    /// - The value cannot be deserialized into the requested type
    /// - There is an error accessing the keyring
    pub fn get_secret<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<T, ConfigError> {
        // First check environment variables (convert to uppercase)
        let env_key = key.to_uppercase();
        if let Ok(val) = env::var(&env_key) {
            let value = Self::parse_env_value(&val)?;
            return Ok(serde_json::from_value(value)?);
        }

        // Then check keyring
        let values = self.all_secrets()?;
        values
            .get(key)
            .ok_or_else(|| ConfigError::NotFound(key.to_string()))
            .and_then(|v| Ok(serde_json::from_value(v.clone())?))
    }

    /// Get secrets. If primary is in env, use env for all keys. Otherwise, use secret storage.
    pub fn get_secrets(
        &self,
        primary: &str,
        maybe_secret: &[&str],
    ) -> Result<HashMap<String, String>, ConfigError> {
        let use_env = env::var(primary.to_uppercase()).is_ok();
        let get_value = |key: &str| -> Result<String, ConfigError> {
            if use_env {
                env::var(key.to_uppercase()).map_err(|_| ConfigError::NotFound(key.to_string()))
            } else {
                self.get_secret(key)
            }
        };

        let mut result = HashMap::new();
        result.insert(primary.to_string(), get_value(primary)?);
        for &key in maybe_secret {
            if let Ok(v) = get_value(key) {
                result.insert(key.to_string(), v);
            }
        }
        Ok(result)
    }

    fn load_secrets_from_storage(&self) -> Result<HashMap<String, Value>, ConfigError> {
        match &self.secrets {
            #[cfg(feature = "system-keyring")]
            SecretStorage::Keyring { service } => {
                let result =
                    self.handle_keyring_operation(|entry| entry.get_password(), service, None);

                match result {
                    Ok(content) => Ok(serde_json::from_str(&content)?),
                    Err(ConfigError::FallbackToFileStorage) => self.fallback_to_file_storage(),
                    Err(ConfigError::KeyringError(msg))
                        if msg.contains("No entry found")
                            || msg.contains("No matching entry found") =>
                    {
                        self.fallback_to_file_storage()
                    }
                    Err(e) => Err(e),
                }
            }
            SecretStorage::File { path } => self.read_secrets_from_file(path),
        }
    }

    fn secrets_mutation_lock_path(&self) -> Result<PathBuf, ConfigError> {
        let storage_path = match &self.secrets {
            #[cfg(feature = "system-keyring")]
            SecretStorage::Keyring { .. } => Self::secrets_file_path(),
            SecretStorage::File { path } => path.clone(),
        };
        Ok(secrets_lock_path(&private_file_target_path(&storage_path)?))
    }

    fn lock_secrets_for_mutation(&self) -> Result<std::fs::File, ConfigError> {
        let lock_path = self.secrets_mutation_lock_path()?;
        if let Some(parent) = lock_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError::DirectoryError(e.to_string()))?;
        }

        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock_file
            .lock_exclusive()
            .map_err(|e| ConfigError::LockError(e.to_string()))?;
        Ok(lock_file)
    }

    fn write_all_secrets(&self, values: &HashMap<String, Value>) -> Result<(), ConfigError> {
        match &self.secrets {
            #[cfg(feature = "system-keyring")]
            SecretStorage::Keyring { service } => {
                let json_value = serde_json::to_string(values)?;
                match self.handle_keyring_operation(
                    |entry| entry.set_password(&json_value),
                    service,
                    Some(values),
                ) {
                    Ok(_) => {}
                    Err(ConfigError::FallbackToFileStorage) => {}
                    Err(e) => return Err(e),
                }
            }
            SecretStorage::File { path } => {
                let yaml_value = serde_yaml::to_string(values)?;
                write_secrets_file(path, &yaml_value)?;
            }
        }

        self.invalidate_secrets_cache();
        Ok(())
    }

    fn mutate_secrets<T>(
        &self,
        mutate: impl FnOnce(&mut HashMap<String, Value>) -> Result<SecretMutation<T>, ConfigError>,
    ) -> Result<T, ConfigError> {
        let _guard = self.guard.lock().unwrap();
        let _storage_lock = self.lock_secrets_for_mutation()?;
        let mut values = self.load_secrets_from_storage()?;
        match mutate(&mut values)? {
            SecretMutation::Write(result) => {
                self.write_all_secrets(&values)?;
                Ok(result)
            }
            SecretMutation::Unchanged(result) => Ok(result),
        }
    }

    /// Set a secret value in the system keyring.
    ///
    /// This will store the value in a single JSON object in the system keyring,
    /// alongside any other secrets. The value can be any type that can be
    /// serialized to JSON.
    ///
    /// Note that this does not affect environment variables - those can only
    /// be set through the system environment.
    ///
    /// # Errors
    ///
    /// Returns a ConfigError if:
    /// - There is an error accessing the keyring
    /// - There is an error serializing the value
    pub fn set_secret<V>(&self, key: &str, value: &V) -> Result<(), ConfigError>
    where
        V: Serialize,
    {
        let value = serde_json::to_value(value)?;
        self.mutate_secrets(|values| {
            values.insert(key.to_string(), value);
            Ok(SecretMutation::Write(()))
        })
    }

    pub(crate) fn update_secret<T, V, R>(
        &self,
        key: &str,
        update: impl FnOnce(T) -> SecretUpdate<V, R>,
    ) -> Result<R, ConfigError>
    where
        T: for<'de> Deserialize<'de> + Default,
        V: Serialize,
    {
        self.mutate_secrets(|values| {
            let current = values
                .get(key)
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            match update(current) {
                SecretUpdate::Write(updated, result) => {
                    values.insert(key.to_string(), serde_json::to_value(updated)?);
                    Ok(SecretMutation::Write(result))
                }
                SecretUpdate::Unchanged(result) => Ok(SecretMutation::Unchanged(result)),
            }
        })
    }

    /// Set multiple secret values with one storage read and one storage write.
    ///
    /// This is intended for provider setup flows that save several fields at once.
    /// It keeps keychain access batched while preserving the same storage format as
    /// `set_secret`.
    pub fn set_secret_values(&self, updates: &[(String, Value)]) -> Result<(), ConfigError> {
        if updates.is_empty() {
            return Ok(());
        }

        self.mutate_secrets(|values| {
            for (key, value) in updates {
                values.insert(key.clone(), value.clone());
            }
            Ok(SecretMutation::Write(()))
        })
    }

    /// Delete a secret from the system keyring.
    ///
    /// This will remove the specified key from the JSON object in the system keyring.
    /// Other secrets will remain unchanged.
    ///
    /// # Errors
    ///
    /// Returns a ConfigError if:
    /// - There is an error accessing the keyring
    /// - There is an error serializing the remaining values
    pub fn delete_secret(&self, key: &str) -> Result<(), ConfigError> {
        self.mutate_secrets(|values| {
            values.remove(key);
            Ok(SecretMutation::Write(()))
        })
    }

    /// Delete multiple secret values with one storage read and one storage write.
    pub fn delete_secret_values(&self, keys: &[String]) -> Result<(), ConfigError> {
        if keys.is_empty() {
            return Ok(());
        }

        self.mutate_secrets(|values| {
            for key in keys {
                values.remove(key);
            }
            Ok(SecretMutation::Write(()))
        })
    }

    /// Read secrets from a YAML file
    fn read_secrets_from_file(&self, path: &Path) -> Result<HashMap<String, Value>, ConfigError> {
        if path.exists() {
            let file_content = std::fs::read_to_string(path)?;
            let yaml_value: serde_yaml::Value = serde_yaml::from_str(&file_content)?;
            let json_value: Value = serde_json::to_value(yaml_value)?;
            match json_value {
                Value::Object(map) => Ok(map.into_iter().collect()),
                _ => Ok(HashMap::new()),
            }
        } else {
            Ok(HashMap::new())
        }
    }

    /// Get the path to the secrets storage file
    #[cfg(feature = "system-keyring")]
    fn secrets_file_path() -> PathBuf {
        secrets_file_path_in(&Paths::config_dir())
    }

    /// Fall back to file storage when keyring is unavailable
    #[cfg(feature = "system-keyring")]
    fn fallback_to_file_storage(&self) -> Result<HashMap<String, Value>, ConfigError> {
        let path = Self::secrets_file_path();
        self.read_secrets_from_file(&path)
    }

    /// Write secrets to file storage (used for fallback)
    #[cfg(feature = "system-keyring")]
    fn write_secrets_to_file(&self, values: &HashMap<String, Value>) -> Result<(), ConfigError> {
        std::fs::create_dir_all(Paths::config_dir())?;
        let path = Self::secrets_file_path();
        let yaml_value = serde_yaml::to_string(values)?;
        write_secrets_file(&path, &yaml_value)?;
        Ok(())
    }

    pub fn invalidate_secrets_cache(&self) {
        let mut cache = self.secrets_cache.lock().unwrap();
        *cache = None;
    }

    /// Check if an error string indicates a keyring availability issue that should trigger fallback
    #[cfg(feature = "system-keyring")]
    fn is_keyring_availability_error(&self, error_str: &str) -> bool {
        let lower = error_str.to_lowercase();
        lower.contains("keyring")
            || lower.contains("dbus")
            || lower.contains("org.freedesktop.secrets")
            || lower.contains("platform secure storage")
            || lower.contains("no secret service")
    }

    /// Get a keyring entry for the specified service
    #[cfg(feature = "system-keyring")]
    fn get_keyring_entry(service: &str) -> Result<keyring::Entry, keyring::Error> {
        Entry::new(service, KEYRING_USERNAME)
    }

    /// Handle keyring errors with automatic fallback to file storage
    #[cfg(feature = "system-keyring")]
    fn handle_keyring_fallback_error<T>(
        &self,
        keyring_err: &keyring::Error,
        fallback_values: Option<&HashMap<String, Value>>,
    ) -> Result<T, ConfigError> {
        if self.is_keyring_availability_error(&keyring_err.to_string()) {
            std::env::set_var("GOOSE_DISABLE_KEYRING", "1");
            tracing::warn!("Keyring unavailable. Using file storage for secrets.");

            if let Some(values) = fallback_values {
                self.write_secrets_to_file(values)?;
                Err(ConfigError::FallbackToFileStorage)
            } else {
                Err(ConfigError::FallbackToFileStorage)
            }
        } else {
            Err(ConfigError::KeyringError(keyring_err.to_string()))
        }
    }

    /// Handle keyring operation with automatic fallback to file storage
    #[cfg(feature = "system-keyring")]
    fn handle_keyring_operation<T>(
        &self,
        operation: impl FnOnce(keyring::Entry) -> Result<T, keyring::Error>,
        service: &str,
        fallback_values: Option<&HashMap<String, Value>>,
    ) -> Result<T, ConfigError> {
        // Try to get the keyring entry and perform the operation
        let entry = match Self::get_keyring_entry(service) {
            Ok(entry) => entry,
            Err(keyring_err) => {
                return self.handle_keyring_fallback_error(&keyring_err, fallback_values);
            }
        };

        // Perform the operation
        match operation(entry) {
            Ok(result) => Ok(result),
            Err(keyring_err) => self.handle_keyring_fallback_error(&keyring_err, fallback_values),
        }
    }
}

config_value!(CLAUDE_CODE_COMMAND, String, "claude");
config_value!(GEMINI_CLI_COMMAND, String, "gemini");
config_value!(CURSOR_AGENT_COMMAND, String, "cursor-agent");
config_value!(CODEX_COMMAND, String, "codex");
config_value!(CODEX_ENABLE_SKILLS, String, "true");
config_value!(CODEX_SKIP_GIT_CHECK, String, "false");
config_value!(CHATGPT_CODEX_REASONING_EFFORT, String, "medium");

config_value!(GOOSE_SEARCH_PATHS, Vec<String>);
config_value!(GOOSE_MODE, GooseMode);
impl Config {
    pub(crate) fn get_goose_mode_strict(&self) -> Result<GooseMode, ConfigError> {
        match env::var("GOOSE_MODE") {
            Ok(value) => {
                let value = Self::parse_env_value(&value)?;
                Ok(serde_json::from_value(value)?)
            }
            Err(env::VarError::NotPresent) => {
                let values = self.load_strict()?;
                let value = values
                    .get("GOOSE_MODE")
                    .ok_or_else(|| ConfigError::NotFound("GOOSE_MODE".to_string()))?;
                Ok(serde_yaml::from_value(value.clone())?)
            }
            Err(env::VarError::NotUnicode(_)) => Err(ConfigError::DeserializeError(
                "GOOSE_MODE contains non-Unicode data".to_string(),
            )),
        }
    }
}
// GOOSE_PROVIDER and GOOSE_MODEL are handled by crate::config::providers
// which checks the structured `providers:` block first and falls back to
// the legacy flat keys. The accessors below delegate to that module.
impl Config {
    pub fn get_goose_provider(&self) -> Result<String, ConfigError> {
        crate::config::providers::get_active_provider(self)
            .ok_or_else(|| ConfigError::NotFound("GOOSE_PROVIDER".to_string()))
    }
    pub fn set_goose_provider(&self, v: impl Into<String>) -> Result<(), ConfigError> {
        let name = v.into();
        let model = crate::config::providers::get_provider_entry(self, &name)
            .map(|e| e.model)
            .unwrap_or_default();
        crate::config::providers::set_active_provider(self, &name, &model)
    }
    pub fn get_goose_model(&self) -> Result<String, ConfigError> {
        crate::config::providers::get_active_model(self)
            .ok_or_else(|| ConfigError::NotFound("GOOSE_MODEL".to_string()))
    }
    pub fn set_goose_model(&self, v: impl Into<String>) -> Result<(), ConfigError> {
        let model = v.into();
        if let Some(provider) = crate::config::providers::get_active_provider(self) {
            crate::config::providers::set_active_provider(self, &provider, &model)?;
        }
        Ok(())
    }
}
config_value!(GOOSE_PROMPT_EDITOR, Option<String>);
config_value!(GOOSE_PROMPT_EDITOR_ALWAYS, Option<bool>);
config_value!(GOOSE_MAX_ACTIVE_AGENTS, usize);
config_value!(GOOSE_DISABLE_SESSION_NAMING, bool);

impl Config {
    pub fn get_goose_context_limit(&self) -> Result<Option<usize>, ConfigError> {
        match self.get_param::<usize>("GOOSE_CONTEXT_LIMIT") {
            Ok(0) => Err(ConfigError::DeserializeError(
                "GOOSE_CONTEXT_LIMIT must be greater than 0".to_string(),
            )),
            Ok(limit) => Ok(Some(limit)),
            Err(ConfigError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn get_goose_max_tokens(&self) -> Result<Option<i32>, ConfigError> {
        match self.get_param::<i32>("GOOSE_MAX_TOKENS") {
            Ok(tokens) if tokens <= 0 => Err(ConfigError::DeserializeError(
                "GOOSE_MAX_TOKENS must be greater than 0".to_string(),
            )),
            Ok(tokens) => Ok(Some(tokens)),
            Err(ConfigError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn get_goose_docs_root(&self) -> Result<Option<String>, ConfigError> {
        match self.get_param::<String>("GOOSE_DOCS_ROOT") {
            Ok(root) => Ok(Some(root.trim().to_string()).filter(|root| !root.is_empty())),
            Err(ConfigError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn get_goose_thinking_effort(&self) -> Option<ThinkingEffort> {
        self.get_param::<String>("GOOSE_THINKING_EFFORT")
            .ok()
            .and_then(|e| e.parse().ok())
            .or_else(|| self.legacy_thinking_effort())
    }

    pub fn set_goose_thinking_effort(&self, v: ThinkingEffort) -> Result<(), ConfigError> {
        self.set_param("GOOSE_THINKING_EFFORT", v)
    }

    pub fn get_openai_store(&self) -> Option<bool> {
        self.get_param::<bool>("OPENAI_STORE").ok()
    }

    fn legacy_thinking_effort(&self) -> Option<ThinkingEffort> {
        if let Ok(value) = self.get_param::<String>("CLAUDE_THINKING_TYPE") {
            if let Some(effort) = match value.to_lowercase().as_str() {
                "adaptive" | "enabled" => Some(ThinkingEffort::High),
                "disabled" => Some(ThinkingEffort::Off),
                _ => None,
            } {
                return Some(effort);
            }
        }

        if let Ok(enabled) = self.get_param::<bool>("CLAUDE_THINKING_ENABLED") {
            return Some(if enabled {
                ThinkingEffort::High
            } else {
                ThinkingEffort::Off
            });
        }

        if let Ok(value) = self.get_param::<String>("GEMINI3_THINKING_LEVEL") {
            if let Some(effort) = Self::legacy_gemini3_thinking_effort(&value) {
                return Some(effort);
            }
        }

        None
    }

    fn legacy_gemini3_thinking_effort(value: &str) -> Option<ThinkingEffort> {
        match value.to_lowercase().as_str() {
            "low" => Some(ThinkingEffort::Low),
            "high" => Some(ThinkingEffort::High),
            _ => None,
        }
    }
}

config_value!(GOOSE_DEFAULT_EXTENSION_TIMEOUT, u64);

fn find_workspace_or_exe_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?.to_path_buf();

    let mut path = exe;
    while let Some(parent) = path.parent() {
        let cargo_toml = parent.join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if content.contains("[workspace]") {
                    return Some(parent.to_path_buf());
                }
            }
        }
        path = parent.to_path_buf();
    }

    Some(exe_dir)
}

pub fn load_init_config_from_workspace() -> Result<Mapping, ConfigError> {
    let root = find_workspace_or_exe_root().ok_or_else(|| {
        ConfigError::FileError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine executable path",
        ))
    })?;

    let init_config_path = root.join("init-config.yaml");
    if !init_config_path.exists() {
        return Err(ConfigError::NotFound(
            "init-config.yaml not found".to_string(),
        ));
    }

    let init_content = std::fs::read_to_string(&init_config_path)?;
    parse_yaml_content(&init_content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::{NamedTempFile, TempDir};
    #[test]
    fn test_basic_config() -> Result<(), ConfigError> {
        let config = new_test_config();

        // Set a simple string value
        config.set_param("test_key", "test_value")?;

        // Test simple string retrieval
        let value: String = config.get_param("test_key")?;
        assert_eq!(value, "test_value");

        // Test with environment variable override
        std::env::set_var("TEST_KEY", "env_value");
        let value: String = config.get_param("test_key")?;
        assert_eq!(value, "env_value");

        Ok(())
    }

    #[test]
    fn test_complex_type() -> Result<(), ConfigError> {
        #[derive(Deserialize, Debug, PartialEq)]
        struct TestStruct {
            field1: String,
            field2: i32,
        }

        let config = new_test_config();

        // Set a complex value
        config.set_param(
            "complex_key",
            serde_json::json!({
                "field1": "hello",
                "field2": 42
            }),
        )?;

        let value: TestStruct = config.get_param("complex_key")?;
        assert_eq!(value.field1, "hello");
        assert_eq!(value.field2, 42);

        Ok(())
    }

    #[test]
    fn test_missing_value() {
        let config = new_test_config();

        let result: Result<String, ConfigError> = config.get_param("nonexistent_key");
        assert!(matches!(result, Err(ConfigError::NotFound(_))));
    }

    #[test]
    fn test_get_param_reads_numeric_yaml_as_u64() -> Result<(), ConfigError> {
        let _guard = env_lock::lock_env([("XXX_TIMEOUT", None::<&str>)]);
        let config = new_test_config();

        config.set_param("XXX_TIMEOUT", 300_u64)?;

        let value: u64 = config.get_param("XXX_TIMEOUT")?;
        assert_eq!(value, 300);
        Ok(())
    }

    #[test]
    fn test_get_param_reads_quoted_numeric_yaml_as_u64() -> Result<(), ConfigError> {
        let _guard = env_lock::lock_env([("XXX_TIMEOUT", None::<&str>)]);
        let config = new_test_config();

        config.set_param("XXX_TIMEOUT", "300")?;

        let value: u64 = config.get_param("XXX_TIMEOUT")?;
        assert_eq!(value, 300);
        Ok(())
    }

    #[test]
    fn test_get_param_reads_quoted_numeric_yaml_as_string() -> Result<(), ConfigError> {
        let _guard = env_lock::lock_env([("XXX_TIMEOUT", None::<&str>)]);
        let config = new_test_config();

        config.set_param("XXX_TIMEOUT", "300")?;

        let value: String = config.get_param("XXX_TIMEOUT")?;
        assert_eq!(value, "300");
        Ok(())
    }

    #[test]
    fn test_get_param_rejects_invalid_string_as_u64() -> Result<(), ConfigError> {
        let _guard = env_lock::lock_env([("XXX_TIMEOUT", None::<&str>)]);
        let config = new_test_config();

        config.set_param("XXX_TIMEOUT", "invalid")?;

        let result: Result<u64, ConfigError> = config.get_param("XXX_TIMEOUT");
        assert!(matches!(result, Err(ConfigError::DeserializeError(_))));
        Ok(())
    }

    #[test]
    fn test_yaml_formatting() -> Result<(), ConfigError> {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        let config = Config::new_with_file_secrets(config_file.path(), secrets_file.path())?;

        config.set_param("key1", "value1")?;
        config.set_param("key2", 42)?;

        // Read the file directly to check YAML formatting
        let content = std::fs::read_to_string(config_file.path())?;
        assert!(content.contains("key1: value1"));
        assert!(content.contains("key2: 42"));

        Ok(())
    }

    #[test]
    fn test_value_management() -> Result<(), ConfigError> {
        let config = new_test_config();

        config.set_param("test_key", "test_value")?;
        config.set_param("another_key", 42)?;
        config.set_param("third_key", true)?;

        let _values = config.load()?;

        let result: Result<String, ConfigError> = config.get_param("key");
        assert!(matches!(result, Err(ConfigError::NotFound(_))));

        Ok(())
    }

    #[test]
    fn test_file_based_secrets_management() -> Result<(), ConfigError> {
        let config = new_test_config();

        config.set_secret("key", &"value")?;

        let value: String = config.get_secret("key")?;
        assert_eq!(value, "value");

        config.delete_secret("key")?;

        let result: Result<String, ConfigError> = config.get_secret("key");
        assert!(matches!(result, Err(ConfigError::NotFound(_))));

        Ok(())
    }

    #[test]
    #[serial]
    fn test_secret_management() -> Result<(), ConfigError> {
        let config = new_test_config();

        // Test setting and getting a simple secret
        config.set_secret("api_key", &Value::String("secret123".to_string()))?;
        let value: String = config.get_secret("api_key")?;
        assert_eq!(value, "secret123");

        // Test environment variable override
        std::env::set_var("API_KEY", "env_secret");
        let value: String = config.get_secret("api_key")?;
        assert_eq!(value, "env_secret");
        std::env::remove_var("API_KEY");

        // Test deleting a secret
        config.delete_secret("api_key")?;
        let result: Result<String, ConfigError> = config.get_secret("api_key");
        assert!(matches!(result, Err(ConfigError::NotFound(_))));

        Ok(())
    }

    #[test]
    fn test_multiple_secrets() -> Result<(), ConfigError> {
        let config = new_test_config();

        // Set multiple secrets
        config.set_secret("key1", &Value::String("secret1".to_string()))?;
        config.set_secret("key2", &Value::String("secret2".to_string()))?;

        // Verify both exist
        let value1: String = config.get_secret("key1")?;
        let value2: String = config.get_secret("key2")?;
        assert_eq!(value1, "secret1");
        assert_eq!(value2, "secret2");

        // Delete one secret
        config.delete_secret("key1")?;

        // Verify key1 is gone but key2 remains
        let result1: Result<String, ConfigError> = config.get_secret("key1");
        let value2: String = config.get_secret("key2")?;
        assert!(matches!(result1, Err(ConfigError::NotFound(_))));
        assert_eq!(value2, "secret2");

        Ok(())
    }

    #[test]
    fn test_secret_mutation_does_not_restore_deleted_secret() -> Result<(), ConfigError> {
        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let first = Config::new_with_file_secrets(&config_path, &secrets_path)?;
        let second = Config::new_with_file_secrets(&config_path, &secrets_path)?;

        first.set_secret("revoked", &"old-token")?;
        first.set_secret("retained", &"retained-value")?;
        let _: String = first.get_secret("revoked")?;

        second.delete_secret("revoked")?;
        first.set_secret("new", &"new-value")?;

        let current = Config::new_with_file_secrets(&config_path, &secrets_path)?;
        assert!(matches!(
            current.get_secret::<String>("revoked"),
            Err(ConfigError::NotFound(_))
        ));
        assert_eq!(current.get_secret::<String>("retained")?, "retained-value");
        assert_eq!(current.get_secret::<String>("new")?, "new-value");

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_secret_mutation_atomically_replaces_storage_file() -> Result<(), ConfigError> {
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;

        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let config = Config::new_with_file_secrets(&config_path, &secrets_path)?;

        config.set_secret("key", &"old-value")?;
        let mut old_file = std::fs::File::open(&secrets_path)?;
        let old_inode = old_file.metadata()?.ino();

        config.set_secret("key", &"new-value")?;

        assert_ne!(std::fs::metadata(&secrets_path)?.ino(), old_inode);
        let mut old_contents = String::new();
        old_file.read_to_string(&mut old_contents)?;
        let old_values: HashMap<String, Value> = serde_yaml::from_str(&old_contents)?;
        assert_eq!(
            old_values.get("key"),
            Some(&Value::String("old-value".into()))
        );
        assert_eq!(config.get_secret::<String>("key")?, "new-value");

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_secret_mutation_lock_uses_resolved_storage_target() -> Result<(), ConfigError> {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let secrets_alias = directory.path().join("secrets-alias.yaml");
        std::fs::write(&secrets_path, "{}\n")?;
        symlink("secrets.yaml", &secrets_alias)?;

        let direct = Config::new_with_file_secrets(&config_path, &secrets_path)?;
        let aliased = Config::new_with_file_secrets(&config_path, &secrets_alias)?;

        assert_eq!(
            direct.secrets_mutation_lock_path()?,
            aliased.secrets_mutation_lock_path()?
        );

        Ok(())
    }

    #[test]
    fn test_secret_reads_remain_cached_across_instances() -> Result<(), ConfigError> {
        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let first = Config::new_with_file_secrets(&config_path, &secrets_path)?;
        let second = Config::new_with_file_secrets(&config_path, &secrets_path)?;

        first.set_secret("key", &"initial")?;
        assert_eq!(first.get_secret::<String>("key")?, "initial");

        second.set_secret("key", &"updated")?;
        assert_eq!(first.get_secret::<String>("key")?, "initial");

        first.invalidate_secrets_cache();
        assert_eq!(first.get_secret::<String>("key")?, "updated");

        Ok(())
    }

    #[test]
    fn test_concurrent_writes() -> Result<(), ConfigError> {
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;

        let config = Arc::new(new_test_config());
        let barrier = Arc::new(Barrier::new(3)); // For 3 concurrent threads
        let values = Arc::new(Mutex::new(Mapping::new()));
        let mut handles = vec![];

        // Initialize with empty values
        config.save_values(&Default::default())?;

        // Spawn 3 threads that will try to write simultaneously
        for i in 0..3 {
            let config = Arc::clone(&config);
            let barrier = Arc::clone(&barrier);
            let values = Arc::clone(&values);
            let handle = thread::spawn(move || -> Result<(), ConfigError> {
                // Wait for all threads to reach this point
                barrier.wait();

                // Get the lock and update values
                let mut values = values.lock().unwrap();
                values.insert(
                    serde_yaml::to_value(format!("key{}", i)).unwrap(),
                    serde_yaml::to_value(format!("value{}", i)).unwrap(),
                );

                // Write all values
                config.save_values(&values)?;
                Ok(())
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap()?;
        }

        // Verify all values were written correctly
        let final_values = config.all_values()?;

        // Print the final values for debugging
        println!("Final values: {:?}", final_values);

        // Check that our 3 keys are present (migrations may add additional keys like "extensions")
        for i in 0..3 {
            let key = format!("key{}", i);
            let value = format!("value{}", i);
            assert!(
                final_values.contains_key(&key),
                "Missing key {} in final values",
                key
            );
            assert_eq!(
                final_values.get(&key).unwrap(),
                &Value::String(value),
                "Incorrect value for key {}",
                key
            );
        }

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_write_follows_symlink() -> Result<(), ConfigError> {
        use std::os::unix::fs as unix_fs;

        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("real_config.yaml");
        let symlink_path = dir.path().join("config.yaml");

        std::fs::write(&target_path, "{}\n")?;
        unix_fs::symlink(&target_path, &symlink_path)?;

        let secrets_file = NamedTempFile::new().unwrap();
        let config = Config::new_with_file_secrets(&symlink_path, secrets_file.path())?;

        config.set_param("key1", "value1")?;

        let meta = std::fs::symlink_metadata(&symlink_path)?;
        assert!(
            meta.file_type().is_symlink(),
            "config path should remain a symlink"
        );

        let content = std::fs::read_to_string(&symlink_path)?;
        assert!(content.contains("key1: value1"));

        let content = std::fs::read_to_string(&target_path)?;
        assert!(content.contains("key1: value1"));

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_write_fails_on_long_symlink_chain() -> Result<(), ConfigError> {
        use std::os::unix::fs as unix_fs;

        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("real_config.yaml");
        std::fs::write(&target_path, "{}\n")?;

        // config.yaml -> link1.yaml -> real_config.yaml
        // We only allow following one symlink hop. If there's another symlink, we should fail
        // rather than overwrite the intermediate symlink.
        let config_symlink = dir.path().join("config.yaml");
        let link1 = dir.path().join("link1.yaml");
        unix_fs::symlink(&target_path, &link1)?;
        unix_fs::symlink(&link1, &config_symlink)?;

        let secrets_file = NamedTempFile::new().unwrap();
        let config = Config::new_with_file_secrets(&config_symlink, secrets_file.path())?;

        let err = config.set_param("key1", "value1").unwrap_err();
        assert!(
            err.to_string().contains("Too many symlink levels"),
            "unexpected error: {err}"
        );

        let meta = std::fs::symlink_metadata(&config_symlink)?;
        assert!(
            meta.file_type().is_symlink(),
            "config path should remain a symlink"
        );
        let meta = std::fs::symlink_metadata(&link1)?;
        assert!(
            meta.file_type().is_symlink(),
            "intermediate link should remain a symlink"
        );

        Ok(())
    }

    #[test]
    fn test_corrupt_config_skipped_on_read() -> Result<(), ConfigError> {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        let config = Config::new_with_file_secrets(config_file.path(), secrets_file.path())?;

        std::fs::write(config_file.path(), "invalid: yaml: content: [unclosed")?;

        // Reads skip corrupt files gracefully
        let values = config.all_values()?;
        assert!(values.is_empty() || !values.contains_key("key1"));

        // A write starts fresh (corrupt content is discarded)
        config.set_param("recovery_key", "value")?;
        let reloaded = config.all_values()?;
        assert!(reloaded.contains_key("recovery_key"));

        Ok(())
    }

    #[test]
    fn test_missing_config_created_on_write() -> Result<(), ConfigError> {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        let config_path = config_file.path().to_path_buf();
        let config = Config::new_with_file_secrets(&config_path, secrets_file.path())?;

        std::fs::remove_file(&config_path)?;
        assert!(!config_path.exists());

        // Reads return empty when file is missing
        let values = config.all_values()?;
        assert!(values.is_empty() || !values.contains_key("key1"));

        // A write creates the file
        config.set_param("new_key", "new_value")?;
        assert!(config_path.exists());

        let file_content = std::fs::read_to_string(&config_path)?;
        let parsed: serde_yaml::Value = serde_yaml::from_str(&file_content)?;
        assert!(parsed.is_mapping());

        Ok(())
    }

    #[test]
    fn test_atomic_write_prevents_corruption() -> Result<(), ConfigError> {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        let config = Config::new_with_file_secrets(config_file.path(), secrets_file.path())?;

        // Set initial values
        config.set_param("key1", "value1")?;

        // Verify the config file exists and is valid
        assert!(config_file.path().exists());
        let content = std::fs::read_to_string(config_file.path())?;
        assert!(serde_yaml::from_str::<serde_yaml::Value>(&content).is_ok());

        // The temp file should not exist after successful write
        let temp_path = config_file.path().with_extension("tmp");
        assert!(!temp_path.exists(), "Temporary file should be cleaned up");

        Ok(())
    }

    #[test]
    fn test_env_var_parsing_strings() -> Result<(), ConfigError> {
        // Test unquoted strings
        let value = Config::parse_env_value("ANTHROPIC")?;
        assert_eq!(value, Value::String("ANTHROPIC".to_string()));

        // Test strings with spaces
        let value = Config::parse_env_value("hello world")?;
        assert_eq!(value, Value::String("hello world".to_string()));

        // Test JSON quoted strings
        let value = Config::parse_env_value("\"ANTHROPIC\"")?;
        assert_eq!(value, Value::String("ANTHROPIC".to_string()));

        // Test empty string
        let value = Config::parse_env_value("")?;
        assert_eq!(value, Value::String("".to_string()));

        Ok(())
    }

    #[test]
    fn test_env_var_parsing_numbers() -> Result<(), ConfigError> {
        // Test integers
        let value = Config::parse_env_value("42")?;
        assert_eq!(value, Value::Number(42.into()));

        let value = Config::parse_env_value("-123")?;
        assert_eq!(value, Value::Number((-123).into()));

        // Test floats
        let value = Config::parse_env_value("3.41")?;
        assert!(matches!(value, Value::Number(_)));
        if let Value::Number(n) = value {
            assert_eq!(n.as_f64().unwrap(), 3.41);
        }

        let value = Config::parse_env_value("0.01")?;
        assert!(matches!(value, Value::Number(_)));
        if let Value::Number(n) = value {
            assert_eq!(n.as_f64().unwrap(), 0.01);
        }

        // Test zero
        let value = Config::parse_env_value("0")?;
        assert_eq!(value, Value::Number(0.into()));

        let value = Config::parse_env_value("0.0")?;
        assert!(matches!(value, Value::Number(_)));
        if let Value::Number(n) = value {
            assert_eq!(n.as_f64().unwrap(), 0.0);
        }

        // Test numbers starting with decimal point
        let value = Config::parse_env_value(".5")?;
        assert!(matches!(value, Value::Number(_)));
        if let Value::Number(n) = value {
            assert_eq!(n.as_f64().unwrap(), 0.5);
        }

        let value = Config::parse_env_value(".00001")?;
        assert!(matches!(value, Value::Number(_)));
        if let Value::Number(n) = value {
            assert_eq!(n.as_f64().unwrap(), 0.00001);
        }

        Ok(())
    }

    #[test]
    fn test_env_var_parsing_booleans() -> Result<(), ConfigError> {
        // Test true variants
        let value = Config::parse_env_value("true")?;
        assert_eq!(value, Value::Bool(true));

        let value = Config::parse_env_value("True")?;
        assert_eq!(value, Value::Bool(true));

        let value = Config::parse_env_value("TRUE")?;
        assert_eq!(value, Value::Bool(true));

        // Test false variants
        let value = Config::parse_env_value("false")?;
        assert_eq!(value, Value::Bool(false));

        let value = Config::parse_env_value("False")?;
        assert_eq!(value, Value::Bool(false));

        let value = Config::parse_env_value("FALSE")?;
        assert_eq!(value, Value::Bool(false));

        Ok(())
    }

    #[test]
    fn test_env_var_parsing_json() -> Result<(), ConfigError> {
        // Test JSON objects
        let value = Config::parse_env_value("{\"host\": \"localhost\", \"port\": 8080}")?;
        assert!(matches!(value, Value::Object(_)));
        if let Value::Object(obj) = value {
            assert_eq!(
                obj.get("host"),
                Some(&Value::String("localhost".to_string()))
            );
            assert_eq!(obj.get("port"), Some(&Value::Number(8080.into())));
        }

        // Test JSON arrays
        let value = Config::parse_env_value("[1, 2, 3]")?;
        assert!(matches!(value, Value::Array(_)));
        if let Value::Array(arr) = value {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Number(1.into()));
            assert_eq!(arr[1], Value::Number(2.into()));
            assert_eq!(arr[2], Value::Number(3.into()));
        }

        // Test JSON null
        let value = Config::parse_env_value("null")?;
        assert_eq!(value, Value::Null);

        Ok(())
    }

    #[test]
    fn test_env_var_parsing_edge_cases() -> Result<(), ConfigError> {
        // Test whitespace handling
        let value = Config::parse_env_value(" 42 ")?;
        assert_eq!(value, Value::Number(42.into()));

        let value = Config::parse_env_value(" true ")?;
        assert_eq!(value, Value::Bool(true));

        // Test strings that look like numbers but aren't
        let value = Config::parse_env_value("123abc")?;
        assert_eq!(value, Value::String("123abc".to_string()));

        let value = Config::parse_env_value("abc123")?;
        assert_eq!(value, Value::String("abc123".to_string()));

        // Test strings that look like booleans but aren't
        let value = Config::parse_env_value("truthy")?;
        assert_eq!(value, Value::String("truthy".to_string()));

        let value = Config::parse_env_value("falsy")?;
        assert_eq!(value, Value::String("falsy".to_string()));

        Ok(())
    }

    #[test]
    fn test_env_var_parsing_numeric_edge_cases() -> Result<(), ConfigError> {
        // Test leading zeros (should be treated as integers, not octal)
        let value = Config::parse_env_value("007")?;
        assert_eq!(value, Value::Number(7.into()));

        // Test large numbers
        let value = Config::parse_env_value("9223372036854775807")?; // i64::MAX
        assert_eq!(value, Value::Number(9223372036854775807i64.into()));

        // Test scientific notation (JSON parsing should handle this correctly)
        let value = Config::parse_env_value("1e10")?;
        assert!(matches!(value, Value::Number(_)));
        if let Value::Number(n) = value {
            assert_eq!(n.as_f64().unwrap(), 1e10);
        }

        // Test infinity (should be treated as string)
        let value = Config::parse_env_value("inf")?;
        assert_eq!(value, Value::String("inf".to_string()));

        Ok(())
    }

    #[test]
    fn test_env_var_with_config_integration() -> Result<(), ConfigError> {
        let config = new_test_config();

        // Test string environment variable (the original issue case)
        std::env::set_var("PROVIDER", "ANTHROPIC");
        let value: String = config.get_param("provider")?;
        assert_eq!(value, "ANTHROPIC");

        // Test number environment variable
        std::env::set_var("PORT", "8080");
        let value: i32 = config.get_param("port")?;
        assert_eq!(value, 8080);

        // Test boolean environment variable
        std::env::set_var("ENABLED", "true");
        let value: bool = config.get_param("enabled")?;
        assert!(value);

        // Test JSON object environment variable
        std::env::set_var("CONFIG", "{\"debug\": true, \"level\": 5}");
        #[derive(Deserialize, Debug, PartialEq)]
        struct TestConfig {
            debug: bool,
            level: i32,
        }
        let value: TestConfig = config.get_param("config")?;
        assert!(value.debug);
        assert_eq!(value.level, 5);

        // Clean up
        std::env::remove_var("PROVIDER");
        std::env::remove_var("PORT");
        std::env::remove_var("ENABLED");
        std::env::remove_var("CONFIG");

        Ok(())
    }

    #[test]
    fn test_env_var_precedence_over_config_file() -> Result<(), ConfigError> {
        let config = new_test_config();

        // Set value in config file
        config.set_param("test_precedence", "file_value")?;

        // Verify file value is returned when no env var
        let value: String = config.get_param("test_precedence")?;
        assert_eq!(value, "file_value");

        // Set environment variable
        std::env::set_var("TEST_PRECEDENCE", "env_value");

        // Environment variable should take precedence
        let value: String = config.get_param("test_precedence")?;
        assert_eq!(value, "env_value");

        // Clean up
        std::env::remove_var("TEST_PRECEDENCE");

        Ok(())
    }

    #[test]
    fn get_secrets_primary_from_env_uses_env_for_secondary() {
        let _guard = env_lock::lock_env([
            ("TEST_PRIMARY", Some("primary_env")),
            ("TEST_SECONDARY", Some("secondary_env")),
        ]);
        let config = new_test_config();
        let secrets = config
            .get_secrets("TEST_PRIMARY", &["TEST_SECONDARY"])
            .unwrap();

        assert_eq!(secrets["TEST_PRIMARY"], "primary_env");
        assert_eq!(secrets["TEST_SECONDARY"], "secondary_env");
    }

    #[test]
    fn get_secrets_primary_from_secret_uses_secret_for_secondary() {
        let _guard = env_lock::lock_env([("TEST_PRIMARY", None::<&str>), ("TEST_SECONDARY", None)]);
        let config = new_test_config();
        config
            .set_secret("TEST_PRIMARY", &"primary_secret")
            .unwrap();
        config
            .set_secret("TEST_SECONDARY", &"secondary_secret")
            .unwrap();

        let secrets = config
            .get_secrets("TEST_PRIMARY", &["TEST_SECONDARY"])
            .unwrap();

        assert_eq!(secrets["TEST_PRIMARY"], "primary_secret");
        assert_eq!(secrets["TEST_SECONDARY"], "secondary_secret");
    }

    #[test]
    fn get_secrets_primary_missing_returns_error() {
        let _guard = env_lock::lock_env([("TEST_PRIMARY", None::<&str>)]);
        let config = new_test_config();

        let result = config.get_secrets("TEST_PRIMARY", &[]);

        assert!(matches!(result, Err(ConfigError::NotFound(_))));
    }

    fn new_test_config() -> Config {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        Config::new_with_file_secrets(config_file.path(), secrets_file.path()).unwrap()
    }

    /// Create a test config where `base_content` is a lower-priority layer
    /// and the actual writable config is a separate (initially empty) file.
    fn new_test_config_with_base(base_content: &str) -> (Config, NamedTempFile) {
        let base_file = NamedTempFile::new().unwrap();
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        std::fs::write(base_file.path(), base_content).unwrap();
        let config = Config::new_with_config_paths(
            vec![
                base_file.path().to_path_buf(),
                config_file.path().to_path_buf(),
            ],
            secrets_file.path(),
        )
        .unwrap();
        (config, base_file)
    }

    #[test]
    fn test_defaults_fallback_when_key_not_in_config() -> Result<(), ConfigError> {
        let (config, _defaults) =
            new_test_config_with_base("SECURITY_PROMPT_ENABLED: true\nsome_key: default_val");

        // Key only in defaults → returns defaults value
        let value: bool = config.get_param("SECURITY_PROMPT_ENABLED")?;
        assert!(value);

        let value: String = config.get_param("some_key")?;
        assert_eq!(value, "default_val");

        Ok(())
    }

    #[test]
    #[serial]
    fn test_full_precedence_env_over_config_over_defaults() -> Result<(), ConfigError> {
        let (config, _defaults) = new_test_config_with_base("my_key: from_defaults");

        // Only defaults → returns defaults
        let value: String = config.get_param("my_key")?;
        assert_eq!(value, "from_defaults");

        // Config file overrides defaults
        config.set_param("my_key", "from_config")?;
        let value: String = config.get_param("my_key")?;
        assert_eq!(value, "from_config");

        // Env var overrides config file (and defaults)
        std::env::set_var("MY_KEY", "from_env");
        let value: String = config.get_param("my_key")?;
        assert_eq!(value, "from_env");
        std::env::remove_var("MY_KEY");

        // After removing env var, config file value is back
        let value: String = config.get_param("my_key")?;
        assert_eq!(value, "from_config");

        Ok(())
    }

    #[test]
    fn test_missing_key_returns_not_found() {
        let config = new_test_config();

        let result: Result<String, ConfigError> = config.get_param("nonexistent_key");
        assert!(matches!(result, Err(ConfigError::NotFound(_))));
    }

    #[test]
    fn test_lower_priority_values_not_persisted_on_write() -> Result<(), ConfigError> {
        let (config, _base) = new_test_config_with_base("base_key: base_value");

        // Read a value from the base layer (should work)
        let value: String = config.get_param("base_key")?;
        assert_eq!(value, "base_value");

        // Write a different key to the user config
        config.set_param("user_key", "user_value")?;

        // Read user config file directly - should NOT contain base_key
        let config_path = PathBuf::from(config.path());
        let file_content = std::fs::read_to_string(&config_path)?;
        assert!(
            !file_content.contains("base_key"),
            "Base layer values should not be persisted to user config on write"
        );
        assert!(
            file_content.contains("user_key"),
            "User's key should be in config file"
        );

        // But reading via get_param should still return the base value
        let value: String = config.get_param("base_key")?;
        assert_eq!(value, "base_value");

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_secrets_file_created_with_restricted_permissions() -> Result<(), ConfigError> {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let config_file = NamedTempFile::new().unwrap();
        let secrets_path = dir.path().join("secrets.yaml");

        let config = Config::new_with_file_secrets(config_file.path(), &secrets_path)?;
        config.set_secret("key", &"value")?;

        let mode = std::fs::metadata(&secrets_path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_existing_secrets_file_permissions_tightened_on_write() -> Result<(), ConfigError> {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let config_file = NamedTempFile::new().unwrap();
        let secrets_path = dir.path().join("secrets.yaml");
        std::fs::write(&secrets_path, "existing: old\n")?;
        std::fs::set_permissions(&secrets_path, std::fs::Permissions::from_mode(0o644))?;

        let config = Config::new_with_file_secrets(config_file.path(), &secrets_path)?;
        config.set_secret("key", &"value")?;

        let value: String = config.get_secret("key")?;
        assert_eq!(value, "value");
        let mode = std::fs::metadata(&secrets_path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        Ok(())
    }

    #[test]
    fn test_merge_config_values_basic_override() {
        let mut base = Mapping::new();
        base.insert(
            serde_yaml::Value::String("key1".into()),
            serde_yaml::Value::String("base_value".into()),
        );
        base.insert(
            serde_yaml::Value::String("key2".into()),
            serde_yaml::Value::String("keep_me".into()),
        );

        let mut overlay = Mapping::new();
        overlay.insert(
            serde_yaml::Value::String("key1".into()),
            serde_yaml::Value::String("overlay_value".into()),
        );
        overlay.insert(
            serde_yaml::Value::String("key3".into()),
            serde_yaml::Value::String("new_value".into()),
        );

        merge_config_values(&mut base, overlay);

        assert_eq!(base.get("key1").unwrap().as_str().unwrap(), "overlay_value");
        assert_eq!(base.get("key2").unwrap().as_str().unwrap(), "keep_me");
        assert_eq!(base.get("key3").unwrap().as_str().unwrap(), "new_value");
    }

    #[test]
    fn test_merge_extensions_append_new() {
        let mut base = Mapping::new();
        let mut base_ext = Mapping::new();
        let mut ext_a = Mapping::new();
        ext_a.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(true),
        );
        ext_a.insert(
            serde_yaml::Value::String("type".into()),
            serde_yaml::Value::String("builtin".into()),
        );
        base_ext.insert(
            serde_yaml::Value::String("ext_a".into()),
            serde_yaml::Value::Mapping(ext_a),
        );
        base.insert(
            serde_yaml::Value::String("extensions".into()),
            serde_yaml::Value::Mapping(base_ext),
        );

        let mut overlay = Mapping::new();
        let mut overlay_ext = Mapping::new();
        let mut ext_b = Mapping::new();
        ext_b.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(true),
        );
        ext_b.insert(
            serde_yaml::Value::String("type".into()),
            serde_yaml::Value::String("stdio".into()),
        );
        overlay_ext.insert(
            serde_yaml::Value::String("ext_b".into()),
            serde_yaml::Value::Mapping(ext_b),
        );
        overlay.insert(
            serde_yaml::Value::String("extensions".into()),
            serde_yaml::Value::Mapping(overlay_ext),
        );

        merge_config_values(&mut base, overlay);

        let extensions = base.get("extensions").unwrap().as_mapping().unwrap();
        assert!(extensions.contains_key("ext_a"));
        assert!(extensions.contains_key("ext_b"));
        // ext_a should be unchanged
        let a = extensions.get("ext_a").unwrap().as_mapping().unwrap();
        assert!(a.get("enabled").unwrap().as_bool().unwrap());
    }

    #[test]
    fn test_merge_extensions_partial_override() {
        // Base has ext_a enabled with several fields
        let mut base = Mapping::new();
        let mut base_ext = Mapping::new();
        let mut ext_a = Mapping::new();
        ext_a.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(true),
        );
        ext_a.insert(
            serde_yaml::Value::String("type".into()),
            serde_yaml::Value::String("builtin".into()),
        );
        ext_a.insert(
            serde_yaml::Value::String("name".into()),
            serde_yaml::Value::String("My Extension".into()),
        );
        base_ext.insert(
            serde_yaml::Value::String("my_ext".into()),
            serde_yaml::Value::Mapping(ext_a),
        );
        base.insert(
            serde_yaml::Value::String("extensions".into()),
            serde_yaml::Value::Mapping(base_ext),
        );

        // Overlay just disables it with a partial entry
        let mut overlay = Mapping::new();
        let mut overlay_ext = Mapping::new();
        let mut ext_override = Mapping::new();
        ext_override.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(false),
        );
        overlay_ext.insert(
            serde_yaml::Value::String("my_ext".into()),
            serde_yaml::Value::Mapping(ext_override),
        );
        overlay.insert(
            serde_yaml::Value::String("extensions".into()),
            serde_yaml::Value::Mapping(overlay_ext),
        );

        merge_config_values(&mut base, overlay);

        let extensions = base.get("extensions").unwrap().as_mapping().unwrap();
        let my_ext = extensions.get("my_ext").unwrap().as_mapping().unwrap();

        // enabled should be overridden to false
        assert!(!my_ext.get("enabled").unwrap().as_bool().unwrap());
        // Other fields should be preserved
        assert_eq!(my_ext.get("type").unwrap().as_str().unwrap(), "builtin");
        assert_eq!(
            my_ext.get("name").unwrap().as_str().unwrap(),
            "My Extension"
        );
    }

    #[test]
    fn test_multi_path_config_loading() -> Result<(), ConfigError> {
        let base_file = NamedTempFile::new().unwrap();
        let user_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();

        // Base (system) config
        std::fs::write(
            base_file.path(),
            "GOOSE_PROVIDER: openai\nGOOSE_MODEL: gpt-4\n",
        )
        .unwrap();

        // User config overrides model
        std::fs::write(user_file.path(), "GOOSE_MODEL: gpt-4o\n").unwrap();

        let config = Config::new_with_config_paths(
            vec![
                base_file.path().to_path_buf(),
                user_file.path().to_path_buf(),
            ],
            secrets_file.path(),
        )?;

        // GOOSE_MODEL should be overridden by later config
        let model: String = config.get_param("GOOSE_MODEL")?;
        assert_eq!(model, "gpt-4o");

        // GOOSE_PROVIDER should still come from base
        let provider: String = config.get_param("GOOSE_PROVIDER")?;
        assert_eq!(provider, "openai");

        Ok(())
    }

    #[test]
    fn test_extension_merge_across_configs() -> Result<(), ConfigError> {
        let base_file = NamedTempFile::new().unwrap();
        let local_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();

        // System config (lower priority) has developer extension enabled
        std::fs::write(
            base_file.path(),
            r#"
extensions:
  developer:
    enabled: true
    type: builtin
    name: Developer
    description: "Core developer tools"
"#,
        )
        .unwrap();

        // User config (higher priority / write target) disables developer and adds a new extension
        std::fs::write(
            local_file.path(),
            r#"
extensions:
  developer:
    enabled: false
  my_custom_ext:
    enabled: true
    type: stdio
    name: MyCustom
    cmd: /usr/bin/my-ext
"#,
        )
        .unwrap();

        // local_file is last = write target and highest priority
        let config = Config::new_with_config_paths(
            vec![
                base_file.path().to_path_buf(),
                local_file.path().to_path_buf(),
            ],
            secrets_file.path(),
        )?;

        let values = config.load()?;
        let extensions = values.get("extensions").unwrap().as_mapping().unwrap();

        // developer should be disabled (user config overrides system)
        let dev = extensions.get("developer").unwrap().as_mapping().unwrap();
        assert!(!dev.get("enabled").unwrap().as_bool().unwrap());
        // Fields from the system config should be preserved via merge
        assert!(dev.get("name").is_some());

        // my_custom_ext should be present from user config
        let custom = extensions
            .get("my_custom_ext")
            .unwrap()
            .as_mapping()
            .unwrap();
        assert!(custom.get("enabled").unwrap().as_bool().unwrap());
        assert_eq!(custom.get("type").unwrap().as_str().unwrap(), "stdio");

        Ok(())
    }

    #[test]
    fn test_three_config_layers_ordered() -> Result<(), ConfigError> {
        let system_file = NamedTempFile::new().unwrap();
        let user_file = NamedTempFile::new().unwrap();
        let local_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();

        std::fs::write(system_file.path(), "key: system\n").unwrap();
        std::fs::write(user_file.path(), "key: user\n").unwrap();
        std::fs::write(local_file.path(), "key: local\n").unwrap();

        let config = Config::new_with_config_paths(
            vec![
                system_file.path().to_path_buf(),
                user_file.path().to_path_buf(),
                local_file.path().to_path_buf(),
            ],
            secrets_file.path(),
        )?;

        let value: String = config.get_param("key")?;
        assert_eq!(value, "local");

        Ok(())
    }

    #[test]
    fn test_missing_config_path_is_skipped() -> Result<(), ConfigError> {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();

        std::fs::write(config_file.path(), "key: base\n").unwrap();

        let config = Config::new_with_config_paths(
            vec![
                PathBuf::from("/tmp/nonexistent_goose_config.yaml"),
                config_file.path().to_path_buf(),
            ],
            secrets_file.path(),
        )?;

        let value: String = config.get_param("key")?;
        assert_eq!(value, "base");

        Ok(())
    }

    #[test]
    fn test_merge_providers_append_new() {
        let mut base = Mapping::new();
        let mut base_prov = Mapping::new();
        let mut prov_a = Mapping::new();
        prov_a.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(true),
        );
        prov_a.insert(
            serde_yaml::Value::String("model".into()),
            serde_yaml::Value::String("gpt-4o".into()),
        );
        prov_a.insert(
            serde_yaml::Value::String("configured".into()),
            serde_yaml::Value::Bool(true),
        );
        base_prov.insert(
            serde_yaml::Value::String("openai".into()),
            serde_yaml::Value::Mapping(prov_a),
        );
        base.insert(
            serde_yaml::Value::String("providers".into()),
            serde_yaml::Value::Mapping(base_prov),
        );

        let mut overlay = Mapping::new();
        let mut overlay_prov = Mapping::new();
        let mut prov_b = Mapping::new();
        prov_b.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(true),
        );
        prov_b.insert(
            serde_yaml::Value::String("model".into()),
            serde_yaml::Value::String("claude-3-opus".into()),
        );
        prov_b.insert(
            serde_yaml::Value::String("configured".into()),
            serde_yaml::Value::Bool(true),
        );
        overlay_prov.insert(
            serde_yaml::Value::String("anthropic".into()),
            serde_yaml::Value::Mapping(prov_b),
        );
        overlay.insert(
            serde_yaml::Value::String("providers".into()),
            serde_yaml::Value::Mapping(overlay_prov),
        );

        merge_config_values(&mut base, overlay);

        let providers = base.get("providers").unwrap().as_mapping().unwrap();
        assert!(providers.contains_key("openai"));
        assert!(providers.contains_key("anthropic"));
        // openai should be unchanged
        let a = providers.get("openai").unwrap().as_mapping().unwrap();
        assert!(a.get("enabled").unwrap().as_bool().unwrap());
        assert_eq!(a.get("model").unwrap().as_str().unwrap(), "gpt-4o");
    }

    #[test]
    fn test_merge_providers_partial_override() {
        let mut base = Mapping::new();
        let mut base_prov = Mapping::new();
        let mut prov = Mapping::new();
        prov.insert(
            serde_yaml::Value::String("enabled".into()),
            serde_yaml::Value::Bool(true),
        );
        prov.insert(
            serde_yaml::Value::String("model".into()),
            serde_yaml::Value::String("gpt-4o".into()),
        );
        prov.insert(
            serde_yaml::Value::String("configured".into()),
            serde_yaml::Value::Bool(true),
        );
        base_prov.insert(
            serde_yaml::Value::String("openai".into()),
            serde_yaml::Value::Mapping(prov),
        );
        base.insert(
            serde_yaml::Value::String("providers".into()),
            serde_yaml::Value::Mapping(base_prov),
        );

        // Overlay just changes the model
        let mut overlay = Mapping::new();
        let mut overlay_prov = Mapping::new();
        let mut prov_override = Mapping::new();
        prov_override.insert(
            serde_yaml::Value::String("model".into()),
            serde_yaml::Value::String("gpt-4o-mini".into()),
        );
        overlay_prov.insert(
            serde_yaml::Value::String("openai".into()),
            serde_yaml::Value::Mapping(prov_override),
        );
        overlay.insert(
            serde_yaml::Value::String("providers".into()),
            serde_yaml::Value::Mapping(overlay_prov),
        );

        merge_config_values(&mut base, overlay);

        let providers = base.get("providers").unwrap().as_mapping().unwrap();
        let openai = providers.get("openai").unwrap().as_mapping().unwrap();

        // model should be overridden
        assert_eq!(
            openai.get("model").unwrap().as_str().unwrap(),
            "gpt-4o-mini"
        );
        // Other fields should be preserved
        assert!(openai.get("enabled").unwrap().as_bool().unwrap());
        assert!(openai.get("configured").unwrap().as_bool().unwrap());
    }

    #[test]
    fn get_goose_context_limit_reads_env() {
        let _guard = env_lock::lock_env([("GOOSE_CONTEXT_LIMIT", Some("4096"))]);
        let config = new_test_config();

        assert_eq!(config.get_goose_context_limit().unwrap(), Some(4096));
    }

    #[test]
    fn get_goose_context_limit_reads_quoted_yaml_value() {
        let _guard = env_lock::lock_env([("GOOSE_CONTEXT_LIMIT", None::<&str>)]);
        let config = new_test_config();
        config.set_param("GOOSE_CONTEXT_LIMIT", "200000").unwrap();

        assert_eq!(config.get_goose_context_limit().unwrap(), Some(200_000));
    }

    #[test]
    fn get_goose_context_limit_returns_none_when_not_set() {
        let _guard = env_lock::lock_env([("GOOSE_CONTEXT_LIMIT", None::<&str>)]);
        let config = new_test_config();

        assert_eq!(config.get_goose_context_limit().unwrap(), None);
    }

    #[test]
    fn get_goose_context_limit_rejects_zero() {
        let _guard = env_lock::lock_env([("GOOSE_CONTEXT_LIMIT", Some("0"))]);
        let config = new_test_config();

        assert!(matches!(
            config.get_goose_context_limit().unwrap_err(),
            ConfigError::DeserializeError(_)
        ));
    }

    #[test]
    fn get_goose_max_tokens_reads_env() {
        let _guard = env_lock::lock_env([("GOOSE_MAX_TOKENS", Some("4096"))]);
        let config = new_test_config();

        assert_eq!(config.get_goose_max_tokens().unwrap(), Some(4096));
    }

    #[test]
    fn get_goose_max_tokens_returns_none_when_not_set() {
        let _guard = env_lock::lock_env([("GOOSE_MAX_TOKENS", None::<&str>)]);
        let config = new_test_config();

        assert_eq!(config.get_goose_max_tokens().unwrap(), None);
    }

    #[test]
    fn get_goose_max_tokens_rejects_invalid_values() {
        for value in ["not_a_number", "0", "-100"] {
            let _guard = env_lock::lock_env([("GOOSE_MAX_TOKENS", Some(value))]);
            let config = new_test_config();

            assert!(matches!(
                config.get_goose_max_tokens().unwrap_err(),
                ConfigError::DeserializeError(_)
            ));
        }
    }

    #[test]
    fn get_goose_docs_root_reads_config_file() {
        let _guard = env_lock::lock_env([("GOOSE_DOCS_ROOT", None::<&str>)]);
        let config = new_test_config();
        config
            .set_param("GOOSE_DOCS_ROOT", "/tmp/goose-docs")
            .unwrap();

        assert_eq!(
            config.get_goose_docs_root().unwrap(),
            Some("/tmp/goose-docs".to_string())
        );
    }

    #[test]
    fn get_goose_docs_root_reads_env_value() {
        let _guard = env_lock::lock_env([("GOOSE_DOCS_ROOT", Some("/tmp/env-docs"))]);
        let config = new_test_config();

        assert_eq!(
            config.get_goose_docs_root().unwrap(),
            Some("/tmp/env-docs".to_string())
        );
    }

    #[test]
    fn get_goose_docs_root_returns_none_when_unset() {
        let _guard = env_lock::lock_env([("GOOSE_DOCS_ROOT", None::<&str>)]);
        let config = new_test_config();

        assert_eq!(config.get_goose_docs_root().unwrap(), None);
    }

    #[test]
    fn get_goose_docs_root_ignores_blank_value() {
        let _guard = env_lock::lock_env([("GOOSE_DOCS_ROOT", None::<&str>)]);
        let config = new_test_config();
        config.set_param("GOOSE_DOCS_ROOT", "   ").unwrap();

        assert_eq!(config.get_goose_docs_root().unwrap(), None);
    }

    #[test]
    fn get_goose_thinking_effort_reads_env() {
        let _guard = env_lock::lock_env([
            ("GOOSE_THINKING_EFFORT", Some("high")),
            ("CLAUDE_THINKING_TYPE", None::<&str>),
            ("CLAUDE_THINKING_ENABLED", None::<&str>),
            ("GEMINI3_THINKING_LEVEL", None::<&str>),
        ]);
        let config = new_test_config();

        assert_eq!(
            config.get_goose_thinking_effort(),
            Some(ThinkingEffort::High)
        );
    }

    #[test]
    fn get_goose_thinking_effort_uses_legacy_claude_fallback() {
        for value in ["enabled", "adaptive"] {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", None::<&str>),
                ("CLAUDE_THINKING_TYPE", Some(value)),
                ("CLAUDE_THINKING_ENABLED", None::<&str>),
                ("GEMINI3_THINKING_LEVEL", None::<&str>),
            ]);
            let config = new_test_config();

            assert_eq!(
                config.get_goose_thinking_effort(),
                Some(ThinkingEffort::High)
            );
        }
    }

    #[test]
    fn get_goose_thinking_effort_uses_legacy_gemini3_fallback() {
        let _guard = env_lock::lock_env([
            ("GOOSE_THINKING_EFFORT", None::<&str>),
            ("CLAUDE_THINKING_TYPE", None::<&str>),
            ("CLAUDE_THINKING_ENABLED", None::<&str>),
            ("GEMINI3_THINKING_LEVEL", Some("high")),
        ]);
        let config = new_test_config();

        assert_eq!(
            config.get_goose_thinking_effort(),
            Some(ThinkingEffort::High)
        );
    }

    #[test]
    fn legacy_gemini3_thinking_level_mapping() {
        assert_eq!(
            Config::legacy_gemini3_thinking_effort("low"),
            Some(ThinkingEffort::Low)
        );
        assert_eq!(
            Config::legacy_gemini3_thinking_effort("high"),
            Some(ThinkingEffort::High)
        );
        assert_eq!(Config::legacy_gemini3_thinking_effort("auto"), None);
    }
}
