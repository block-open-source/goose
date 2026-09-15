// Extension data management for sessions
// Provides a simple way to store extension-specific data with versioned keys

use crate::config::base::Config;
use crate::config::extensions::is_extension_available;
use crate::config::extensions::is_extension_enabled;
use crate::config::ExtensionConfig;
use crate::session::SessionManager;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Extension data containing all extension states
/// Keys are in format "extension_name.version" (e.g., "todo.v0")
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtensionData {
    #[serde(flatten)]
    pub extension_states: HashMap<String, Value>,
}

impl ExtensionData {
    /// Create a new empty ExtensionData
    pub fn new() -> Self {
        Self {
            extension_states: HashMap::new(),
        }
    }

    /// Get extension state for a specific extension and version
    pub fn get_extension_state(&self, extension_name: &str, version: &str) -> Option<&Value> {
        let key = format!("{}.{}", extension_name, version);
        self.extension_states.get(&key)
    }

    /// Set extension state for a specific extension and version
    pub fn set_extension_state(&mut self, extension_name: &str, version: &str, state: Value) {
        let key = format!("{}.{}", extension_name, version);
        self.extension_states.insert(key, state);
    }
}

/// Helper trait for extension-specific state management
pub trait ExtensionState: Sized + Serialize + for<'de> Deserialize<'de> {
    /// The name of the extension
    const EXTENSION_NAME: &'static str;

    /// The version of the extension state format
    const VERSION: &'static str;

    /// Convert from JSON value
    fn from_value(value: &Value) -> Result<Self> {
        serde_json::from_value(value.clone()).map_err(|e| {
            anyhow::anyhow!(
                "Failed to deserialize {} state: {}",
                Self::EXTENSION_NAME,
                e
            )
        })
    }

    /// Convert to JSON value
    fn to_value(&self) -> Result<Value> {
        serde_json::to_value(self).map_err(|e| {
            anyhow::anyhow!("Failed to serialize {} state: {}", Self::EXTENSION_NAME, e)
        })
    }

    /// Get state from extension data
    fn from_extension_data(extension_data: &ExtensionData) -> Option<Self> {
        extension_data
            .get_extension_state(Self::EXTENSION_NAME, Self::VERSION)
            .and_then(|v| Self::from_value(v).ok())
    }

    /// Save state to extension data
    fn to_extension_data(&self, extension_data: &mut ExtensionData) -> Result<()> {
        let value = self.to_value()?;
        extension_data.set_extension_state(Self::EXTENSION_NAME, Self::VERSION, value);
        Ok(())
    }
}

/// TODO extension state implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoState {
    pub content: String,
}

impl ExtensionState for TodoState {
    const EXTENSION_NAME: &'static str = "todo";
    const VERSION: &'static str = "v0";
}

impl TodoState {
    /// Create a new TODO state
    pub fn new(content: String) -> Self {
        Self { content }
    }
}

/// Enabled extensions state implementation for storing which extensions are active
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnabledExtensionsState {
    pub extensions: Vec<ExtensionConfig>,
}

impl ExtensionState for EnabledExtensionsState {
    const EXTENSION_NAME: &'static str = "enabled_extensions";
    const VERSION: &'static str = "v0";
}

impl EnabledExtensionsState {
    pub fn new(extensions: Vec<ExtensionConfig>) -> Self {
        Self { extensions }
    }

    pub fn from_extension_data(extension_data: &ExtensionData) -> Option<Self> {
        let mut state = <Self as ExtensionState>::from_extension_data(extension_data)?;
        state
            .extensions
            .retain(|ext| is_extension_available(ext) && is_extension_enabled(&ext.name()));
        Some(state)
    }

    pub fn extensions_or_default(
        extension_data: Option<&ExtensionData>,
        config: &Config,
    ) -> Vec<ExtensionConfig> {
        extension_data
            .and_then(Self::from_extension_data)
            .map(|state| state.extensions)
            .unwrap_or_else(|| {
                crate::config::extensions::get_enabled_extensions_with_config(config)
            })
    }

    pub async fn for_session(
        session_manager: &SessionManager,
        session_id: &str,
        config: &Config,
    ) -> Vec<ExtensionConfig> {
        let session = session_manager.get_session(session_id, false).await.ok();
        Self::extensions_or_default(session.as_ref().map(|s| &s.extension_data), config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::NamedTempFile;
    use test_case::test_case;

    fn test_config() -> Config {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        Config::new_with_file_secrets(config_file.path(), secrets_file.path()).unwrap()
    }

    fn test_extension() -> ExtensionConfig {
        ExtensionConfig::Builtin {
            name: "developer".into(),
            description: "dev".into(),
            display_name: None,
            timeout: None,
            bundled: None,
            available_tools: vec![],
        }
    }

    fn extension_data_with(extensions: Vec<ExtensionConfig>) -> ExtensionData {
        let mut data = ExtensionData::new();
        EnabledExtensionsState::new(extensions)
            .to_extension_data(&mut data)
            .unwrap();
        data
    }

    #[test_case(
        Some(extension_data_with(vec![test_extension()])),
        Some(vec![test_extension()])
        ; "prefers_session_data"
    )]
    #[test_case(None, None ; "no_session_falls_back_to_config")]
    #[test_case(Some(ExtensionData::default()), None ; "empty_session_data_falls_back_to_config")]
    fn test_extensions_or_default(
        extension_data: Option<ExtensionData>,
        expected: Option<Vec<ExtensionConfig>>,
    ) {
        let config = test_config();
        let expected = expected.unwrap_or_else(|| {
            crate::config::extensions::get_enabled_extensions_with_config(&config)
        });
        assert_eq!(
            EnabledExtensionsState::extensions_or_default(extension_data.as_ref(), &config),
            expected,
        );
    }

    #[test]
    fn test_extension_data_basic_operations() {
        let mut extension_data = ExtensionData::new();

        // Test setting and getting extension state
        let todo_state = json!({"content": "- Task 1\n- Task 2"});
        extension_data.set_extension_state("todo", "v0", todo_state.clone());

        assert_eq!(
            extension_data.get_extension_state("todo", "v0"),
            Some(&todo_state)
        );
        assert_eq!(extension_data.get_extension_state("todo", "v1"), None);
    }

    #[test]
    fn test_multiple_extension_states() {
        let mut extension_data = ExtensionData::new();

        // Add multiple extension states
        extension_data.set_extension_state("todo", "v0", json!("TODO content"));
        extension_data.set_extension_state("memory", "v1", json!({"items": ["item1", "item2"]}));
        extension_data.set_extension_state("config", "v2", json!({"setting": true}));

        // Check all states exist
        assert_eq!(extension_data.extension_states.len(), 3);
        assert!(extension_data.get_extension_state("todo", "v0").is_some());
        assert!(extension_data.get_extension_state("memory", "v1").is_some());
        assert!(extension_data.get_extension_state("config", "v2").is_some());
    }

    #[test]
    fn test_todo_state_trait() {
        let mut extension_data = ExtensionData::new();

        // Create and save TODO state
        let todo = TodoState::new("- Task 1\n- Task 2".to_string());
        todo.to_extension_data(&mut extension_data).unwrap();

        // Retrieve TODO state
        let retrieved = TodoState::from_extension_data(&extension_data);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "- Task 1\n- Task 2");
    }

    #[test]
    fn test_extension_data_serialization() {
        let mut extension_data = ExtensionData::new();
        extension_data.set_extension_state("todo", "v0", json!("TODO content"));
        extension_data.set_extension_state("memory", "v1", json!({"key": "value"}));

        // Serialize to JSON
        let json = serde_json::to_value(&extension_data).unwrap();

        // Check the structure
        assert!(json.is_object());
        assert_eq!(json.get("todo.v0"), Some(&json!("TODO content")));
        assert_eq!(json.get("memory.v1"), Some(&json!({"key": "value"})));

        // Deserialize back
        let deserialized: ExtensionData = serde_json::from_value(json).unwrap();
        assert_eq!(
            deserialized.get_extension_state("todo", "v0"),
            Some(&json!("TODO content"))
        );
        assert_eq!(
            deserialized.get_extension_state("memory", "v1"),
            Some(&json!({"key": "value"}))
        );
    }

    #[test]
    fn test_enabled_extensions_state_filters_unavailable_platform() {
        let mut extension_data = ExtensionData::new();
        let state = EnabledExtensionsState::new(vec![
            ExtensionConfig::Platform {
                name: "definitely_not_real_platform_extension".to_string(),
                description: "unknown".to_string(),
                display_name: None,
                bundled: None,
                available_tools: Vec::new(),
            },
            ExtensionConfig::Builtin {
                name: "developer".to_string(),
                description: "".to_string(),
                display_name: Some("Developer".to_string()),
                timeout: None,
                bundled: None,
                available_tools: Vec::new(),
            },
        ]);

        state.to_extension_data(&mut extension_data).unwrap();

        let loaded =
            EnabledExtensionsState::from_extension_data(&extension_data).expect("state present");
        let names: Vec<String> = loaded.extensions.iter().map(|ext| ext.name()).collect();

        assert!(names.iter().any(|name| name == "developer"));
        assert!(!names
            .iter()
            .any(|name| name == "definitely_not_real_platform_extension"));
    }

    #[test]
    fn test_from_extension_data_filters_disabled_extensions() {
        // When an extension was previously enabled, used in a session (leaving
        // state in extension_data), then later disabled in config, the stale
        // state should not be loaded back on session resume.
        //
        // We test this by checking that from_extension_data does not return
        // extensions that are disabled in the global config. The "todo"
        // extension is disabled by default in the test config, so any todo
        // state persisted in extension_data should be filtered out.
        let mut extension_data = ExtensionData::new();
        let state = EnabledExtensionsState::new(vec![
            ExtensionConfig::Builtin {
                name: "developer".to_string(),
                description: "".to_string(),
                display_name: Some("Developer".to_string()),
                timeout: None,
                bundled: None,
                available_tools: Vec::new(),
            },
            ExtensionConfig::Platform {
                name: "todo".to_string(),
                description: "Todo list".to_string(),
                display_name: Some("Todo".to_string()),
                bundled: None,
                available_tools: Vec::new(),
            },
        ]);

        state.to_extension_data(&mut extension_data).unwrap();

        let loaded =
            EnabledExtensionsState::from_extension_data(&extension_data).expect("state present");
        let names: Vec<String> = loaded.extensions.iter().map(|ext| ext.name()).collect();

        // developer is enabled by default in the test config
        assert!(names.iter().any(|name| name == "developer"));
        // todo is disabled by default in the test config, so it should be filtered out
        assert!(
            !names.iter().any(|name| name == "todo"),
            "disabled extension 'todo' should not be loaded from extension_data"
        );
    }
}
