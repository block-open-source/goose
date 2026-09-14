use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::base::SecretUpdate;
use crate::config::Config;

use super::{PairingState, PlatformUser};

const PAIRINGS_CONFIG_KEY: &str = "gateway_pairings";
const PENDING_CODES_CONFIG_KEY: &str = "gateway_pending_codes";
const PENDING_CODES_SECRET_KEY: &str = "gateway_pending_codes";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPairing {
    platform: String,
    user_id: String,
    display_name: Option<String>,
    state: PairingState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPendingCode {
    code: String,
    gateway_type: String,
    expires_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredPendingCodes {
    codes: Vec<StoredPendingCode>,
    #[serde(default)]
    legacy_import_complete: bool,
    #[serde(default)]
    imported_legacy_codes: Vec<String>,
}

impl StoredPendingCodes {
    fn revoke_imported_codes(&mut self) -> bool {
        let previous_len = self.codes.len();
        self.codes.retain(|pending| {
            !self
                .imported_legacy_codes
                .iter()
                .any(|code| code == &pending.code)
        });
        self.codes.len() != previous_len
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredPendingCodesValue {
    State(StoredPendingCodes),
    Codes(Vec<StoredPendingCode>),
}

impl Default for StoredPendingCodesValue {
    fn default() -> Self {
        Self::State(StoredPendingCodes::default())
    }
}

impl StoredPendingCodesValue {
    fn into_state(self) -> StoredPendingCodes {
        match self {
            Self::State(state) => state,
            Self::Codes(codes) => StoredPendingCodes {
                codes,
                legacy_import_complete: false,
                imported_legacy_codes: Vec::new(),
            },
        }
    }
}

pub struct PairingStore {
    pairings: RwLock<HashMap<PlatformUser, PairingState>>,
}

impl PairingStore {
    pub fn new() -> anyhow::Result<Self> {
        let pairings = Self::load_pairings_from_config();
        Ok(Self {
            pairings: RwLock::new(pairings),
        })
    }

    fn load_pairings_from_config() -> HashMap<PlatformUser, PairingState> {
        let config = Config::global();
        let entries: Vec<StoredPairing> = config.get_param(PAIRINGS_CONFIG_KEY).unwrap_or_default();
        let mut map = HashMap::new();
        for entry in entries {
            let user = PlatformUser {
                platform: entry.platform,
                user_id: entry.user_id,
                display_name: entry.display_name,
            };
            map.insert(user, entry.state);
        }
        map
    }

    fn save_pairings_to_config(
        pairings: &HashMap<PlatformUser, PairingState>,
    ) -> anyhow::Result<()> {
        let entries: Vec<StoredPairing> = pairings
            .iter()
            .map(|(user, state)| StoredPairing {
                platform: user.platform.clone(),
                user_id: user.user_id.clone(),
                display_name: user.display_name.clone(),
                state: state.clone(),
            })
            .collect();
        Config::global()
            .set_param(PAIRINGS_CONFIG_KEY, &entries)
            .map_err(|e| anyhow::anyhow!("failed to save gateway pairings: {}", e))
    }

    fn migrate_pending_codes(config: &Config) -> anyhow::Result<()> {
        let legacy_codes: Vec<_> = config
            .get_param_source_values::<Vec<StoredPendingCode>>(PENDING_CODES_CONFIG_KEY)
            .map_err(|error| anyhow::anyhow!("failed to verify legacy pending codes: {error}"))?
            .into_iter()
            .flatten()
            .collect();
        if legacy_codes.is_empty() {
            return Ok(());
        }

        Self::revoke_legacy_pending_codes(config, legacy_codes)?;
        Err(anyhow::anyhow!(
            "legacy pending codes remain in ordinary configuration and have been revoked; stop older Goose processes, remove GATEWAY_PENDING_CODES and gateway_pending_codes from configured files, then retry and generate a new pairing code"
        ))
    }

    fn revoke_legacy_pending_codes(
        config: &Config,
        legacy_codes: Vec<StoredPendingCode>,
    ) -> anyhow::Result<()> {
        config
            .update_secret(
                PENDING_CODES_SECRET_KEY,
                |stored: StoredPendingCodesValue| {
                    let mut state = stored.into_state();
                    let mut changed = !state.legacy_import_complete;
                    for legacy_code in legacy_codes {
                        if state
                            .imported_legacy_codes
                            .iter()
                            .any(|code| code == &legacy_code.code)
                        {
                            continue;
                        }
                        state.imported_legacy_codes.push(legacy_code.code);
                        changed = true;
                    }
                    changed |= state.revoke_imported_codes();
                    state.legacy_import_complete = true;
                    if changed {
                        SecretUpdate::Write(state, ())
                    } else {
                        SecretUpdate::Unchanged(())
                    }
                },
            )
            .map_err(|error| anyhow::anyhow!("failed to migrate pending codes: {}", error))
    }

    fn store_pending_code_in(
        config: &Config,
        code: &str,
        gateway_type: &str,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        Self::migrate_pending_codes(config)?;
        config
            .update_secret(
                PENDING_CODES_SECRET_KEY,
                |stored: StoredPendingCodesValue| {
                    let mut state = stored.into_state();
                    state.revoke_imported_codes();
                    state
                        .imported_legacy_codes
                        .retain(|legacy_code| legacy_code != code);
                    state.legacy_import_complete = true;
                    state.codes.retain(|pending| pending.code != code);
                    state.codes.push(StoredPendingCode {
                        code: code.to_string(),
                        gateway_type: gateway_type.to_string(),
                        expires_at,
                    });
                    SecretUpdate::Write(state, ())
                },
            )
            .map_err(|error| anyhow::anyhow!("failed to save pending codes: {}", error))
    }

    fn consume_pending_code_in(
        config: &Config,
        code: &str,
        now: i64,
    ) -> anyhow::Result<Option<String>> {
        Self::migrate_pending_codes(config)?;
        config
            .update_secret(
                PENDING_CODES_SECRET_KEY,
                |stored: StoredPendingCodesValue| {
                    let mut state = stored.into_state();
                    let mut needs_write = !state.legacy_import_complete;
                    state.legacy_import_complete = true;
                    needs_write |= state.revoke_imported_codes();
                    let Some(position) =
                        state.codes.iter().position(|pending| pending.code == code)
                    else {
                        return if needs_write {
                            SecretUpdate::Write(state, None)
                        } else {
                            SecretUpdate::Unchanged(None)
                        };
                    };
                    let consumed = Some(state.codes.remove(position))
                        .filter(|pending| now <= pending.expires_at)
                        .map(|pending| pending.gateway_type);
                    SecretUpdate::Write(state, consumed)
                },
            )
            .map_err(|error| anyhow::anyhow!("failed to consume pending code: {}", error))
    }

    pub async fn get(&self, user: &PlatformUser) -> anyhow::Result<PairingState> {
        let pairings = self.pairings.read().await;
        Ok(pairings
            .get(user)
            .cloned()
            .unwrap_or(PairingState::Unpaired))
    }

    pub async fn set(&self, user: &PlatformUser, state: PairingState) -> anyhow::Result<()> {
        let mut pairings = self.pairings.write().await;
        pairings.insert(user.clone(), state);
        Self::save_pairings_to_config(&pairings)
    }

    pub async fn remove(&self, user: &PlatformUser) -> anyhow::Result<()> {
        let mut pairings = self.pairings.write().await;
        pairings.remove(user);
        Self::save_pairings_to_config(&pairings)
    }

    pub async fn store_pending_code(
        &self,
        code: &str,
        gateway_type: &str,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        Self::store_pending_code_in(Config::global(), code, gateway_type, expires_at)
    }

    pub async fn consume_pending_code(&self, code: &str) -> anyhow::Result<Option<String>> {
        let now = chrono::Utc::now().timestamp();
        Self::consume_pending_code_in(Config::global(), code, now)
    }

    pub fn generate_code() -> String {
        use rand::RngExt;
        let chars: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut rng = rand::rng();
        (0..6)
            .map(|_| chars[rng.random_range(0..chars.len())] as char)
            .collect()
    }

    pub async fn remove_all_for_platform(&self, platform: &str) -> anyhow::Result<usize> {
        let mut pairings = self.pairings.write().await;
        let before = pairings.len();
        pairings.retain(|user, _| user.platform != platform);
        let removed = before - pairings.len();
        Self::save_pairings_to_config(&pairings)?;
        Ok(removed)
    }

    pub async fn list_paired_users(
        &self,
        gateway_type: &str,
    ) -> anyhow::Result<Vec<(PlatformUser, String, i64)>> {
        let pairings = self.pairings.read().await;
        let mut result = Vec::new();
        for (user, state) in pairings.iter() {
            if user.platform == gateway_type {
                if let PairingState::Paired {
                    session_id,
                    paired_at,
                } = state
                {
                    result.push((user.clone(), session_id.clone(), *paired_at));
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    fn test_config(directory: &TempDir) -> Config {
        Config::new_with_file_secrets(
            directory.path().join("config.yaml"),
            directory.path().join("secrets.yaml"),
        )
        .unwrap()
    }

    #[test]
    fn test_code_generation() {
        let code = PairingStore::generate_code();
        assert_eq!(code.len(), 6);
        assert!(code
            .chars()
            .all(|c| "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(c)));
    }

    #[test]
    fn pending_codes_are_secret_and_consumed_once() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let code = "SECRET-CODE";

        PairingStore::store_pending_code_in(&config, code, "telegram", 101).unwrap();

        let ordinary_config =
            std::fs::read_to_string(directory.path().join("config.yaml")).unwrap_or_default();
        assert!(!ordinary_config.contains(code));
        assert!(!config
            .all_values()
            .unwrap()
            .contains_key(PENDING_CODES_CONFIG_KEY));
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            Some("telegram".to_string())
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            None
        );

        PairingStore::store_pending_code_in(&config, code, "telegram", 99).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            None
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 98).unwrap(),
            None
        );
    }

    #[test]
    fn pending_code_store_revokes_legacy_config_and_requires_cleanup() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let legacy_code = "LEGACY-CODE";
        let new_code = "NEW-CODE";
        config
            .set_param(
                PENDING_CODES_CONFIG_KEY,
                &[StoredPendingCode {
                    code: legacy_code.to_string(),
                    gateway_type: "slack".to_string(),
                    expires_at: 101,
                }],
            )
            .unwrap();

        let error =
            PairingStore::store_pending_code_in(&config, new_code, "telegram", 101).unwrap_err();
        assert!(error.to_string().contains("stop older Goose processes"));

        let ordinary_config =
            std::fs::read_to_string(directory.path().join("config.yaml")).unwrap_or_default();
        assert!(ordinary_config.contains(legacy_code));
        assert!(!ordinary_config.contains(new_code));

        config.delete(PENDING_CODES_CONFIG_KEY).unwrap();
        PairingStore::store_pending_code_in(&config, new_code, "telegram", 101).unwrap();

        let ordinary_config =
            std::fs::read_to_string(directory.path().join("config.yaml")).unwrap_or_default();
        assert!(!ordinary_config.contains(legacy_code));
        assert!(!ordinary_config.contains(new_code));
        assert!(!config
            .all_values()
            .unwrap()
            .contains_key(PENDING_CODES_CONFIG_KEY));
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, legacy_code, 100).unwrap(),
            None
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, new_code, 100).unwrap(),
            Some("telegram".to_string())
        );
    }

    #[test]
    fn pending_code_migration_rejects_non_writable_config_sources() {
        let directory = TempDir::new().unwrap();
        let system_config_path = directory.path().join("system.yaml");
        let user_config_path = directory.path().join("user.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let system_config =
            Config::new_with_file_secrets(&system_config_path, &secrets_path).unwrap();
        let legacy_code = StoredPendingCode {
            code: "SYSTEM-CODE".to_string(),
            gateway_type: "slack".to_string(),
            expires_at: 101,
        };
        system_config
            .set_param(PENDING_CODES_CONFIG_KEY, [&legacy_code])
            .unwrap();
        let config = Config::new_with_config_paths(
            vec![system_config_path, user_config_path.clone()],
            &secrets_path,
        )
        .unwrap();

        let error =
            PairingStore::store_pending_code_in(&config, "NEW-CODE", "telegram", 101).unwrap_err();

        assert!(error
            .to_string()
            .contains("legacy pending codes remain in ordinary configuration"));
        assert_eq!(
            config
                .get_param::<Vec<StoredPendingCode>>(PENDING_CODES_CONFIG_KEY)
                .unwrap()[0]
                .code,
            legacy_code.code
        );
        let user_config = std::fs::read_to_string(user_config_path).unwrap_or_default();
        assert!(!user_config.contains(&legacy_code.code));

        std::fs::write(directory.path().join("system.yaml"), "").unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, &legacy_code.code, 100).unwrap(),
            None
        );
    }

    #[test]
    fn empty_non_writable_legacy_source_does_not_disable_pairing() {
        let directory = TempDir::new().unwrap();
        let system_config_path = directory.path().join("system.yaml");
        let user_config_path = directory.path().join("user.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let system_config =
            Config::new_with_file_secrets(&system_config_path, &secrets_path).unwrap();
        system_config
            .set_param(PENDING_CODES_CONFIG_KEY, Vec::<StoredPendingCode>::new())
            .unwrap();
        let config = Config::new_with_config_paths(
            vec![system_config_path, user_config_path],
            &secrets_path,
        )
        .unwrap();

        PairingStore::store_pending_code_in(&config, "NEW-CODE", "telegram", 101).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "NEW-CODE", 100).unwrap(),
            Some("telegram".to_string())
        );
    }

    #[test]
    fn empty_overlay_cannot_hide_nonempty_legacy_source() {
        let directory = TempDir::new().unwrap();
        let system_config_path = directory.path().join("system.yaml");
        let additional_config_path = directory.path().join("additional.yaml");
        let user_config_path = directory.path().join("user.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let system_config =
            Config::new_with_file_secrets(&system_config_path, &secrets_path).unwrap();
        system_config
            .set_param(
                PENDING_CODES_CONFIG_KEY,
                [StoredPendingCode {
                    code: "SYSTEM-CODE".to_string(),
                    gateway_type: "slack".to_string(),
                    expires_at: 101,
                }],
            )
            .unwrap();
        let additional_config =
            Config::new_with_file_secrets(&additional_config_path, &secrets_path).unwrap();
        additional_config
            .set_param(PENDING_CODES_CONFIG_KEY, Vec::<StoredPendingCode>::new())
            .unwrap();
        let config = Config::new_with_config_paths(
            vec![system_config_path, additional_config_path, user_config_path],
            &secrets_path,
        )
        .unwrap();

        assert!(
            PairingStore::store_pending_code_in(&config, "NEW-CODE", "telegram", 101)
                .unwrap_err()
                .to_string()
                .contains("legacy pending codes remain")
        );
    }

    #[test]
    fn pending_code_migration_revokes_codes_from_every_source() {
        let directory = TempDir::new().unwrap();
        let system_config_path = directory.path().join("system.yaml");
        let user_config_path = directory.path().join("user.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let system_config =
            Config::new_with_file_secrets(&system_config_path, &secrets_path).unwrap();
        system_config
            .set_param(
                PENDING_CODES_CONFIG_KEY,
                [StoredPendingCode {
                    code: "SYSTEM-CODE".to_string(),
                    gateway_type: "slack".to_string(),
                    expires_at: 101,
                }],
            )
            .unwrap();
        let user_config = Config::new_with_file_secrets(&user_config_path, &secrets_path).unwrap();
        user_config
            .set_param(
                PENDING_CODES_CONFIG_KEY,
                [StoredPendingCode {
                    code: "USER-CODE".to_string(),
                    gateway_type: "telegram".to_string(),
                    expires_at: 101,
                }],
            )
            .unwrap();
        let config = Config::new_with_config_paths(
            vec![system_config_path.clone(), user_config_path],
            &secrets_path,
        )
        .unwrap();

        assert!(
            PairingStore::store_pending_code_in(&config, "NEW-CODE", "telegram", 101)
                .unwrap_err()
                .to_string()
                .contains("legacy pending codes remain")
        );

        std::fs::write(system_config_path, "").unwrap();
        config.delete(PENDING_CODES_CONFIG_KEY).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "USER-CODE", 100).unwrap(),
            None
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "SYSTEM-CODE", 100).unwrap(),
            None
        );
    }

    #[test]
    fn pending_code_consumption_is_serialized_across_config_instances() {
        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let config = Config::new_with_file_secrets(&config_path, &secrets_path).unwrap();
        let code = "ONE-TIME-CODE";
        PairingStore::store_pending_code_in(&config, code, "telegram", 101).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let config_path = config_path.clone();
                let secrets_path = secrets_path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let config = Config::new_with_file_secrets(config_path, secrets_path).unwrap();
                    barrier.wait();
                    PairingStore::consume_pending_code_in(&config, code, 100).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert_eq!(results.iter().filter(|result| result.is_some()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_none()).count(), 1);
    }

    #[test]
    fn delayed_legacy_migration_cannot_activate_revoked_code() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let code = "LEGACY-ONE-TIME-CODE";
        config
            .set_param(
                PENDING_CODES_CONFIG_KEY,
                &[StoredPendingCode {
                    code: code.to_string(),
                    gateway_type: "telegram".to_string(),
                    expires_at: 101,
                }],
            )
            .unwrap();
        let stale_snapshot: Vec<StoredPendingCode> =
            config.get_param(PENDING_CODES_CONFIG_KEY).unwrap();

        PairingStore::revoke_legacy_pending_codes(&config, stale_snapshot.clone()).unwrap();
        config.delete(PENDING_CODES_CONFIG_KEY).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            None
        );
        PairingStore::revoke_legacy_pending_codes(&config, stale_snapshot).unwrap();

        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            None
        );
    }

    #[test]
    fn new_legacy_code_after_completed_migration_is_revoked() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        PairingStore::store_pending_code_in(&config, "CURRENT-CODE", "telegram", 101).unwrap();
        assert!(
            config
                .get_secret::<StoredPendingCodes>(PENDING_CODES_SECRET_KEY)
                .unwrap()
                .legacy_import_complete
        );

        config
            .set_param(
                PENDING_CODES_CONFIG_KEY,
                [StoredPendingCode {
                    code: "ROLLBACK-CODE".to_string(),
                    gateway_type: "slack".to_string(),
                    expires_at: 101,
                }],
            )
            .unwrap();

        assert!(
            PairingStore::consume_pending_code_in(&config, "ROLLBACK-CODE", 100)
                .unwrap_err()
                .to_string()
                .contains("stop older Goose processes")
        );
        config.delete(PENDING_CODES_CONFIG_KEY).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "ROLLBACK-CODE", 100).unwrap(),
            None
        );
        PairingStore::store_pending_code_in(&config, "FRESH-CODE", "slack", 101).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "FRESH-CODE", 100).unwrap(),
            Some("slack".to_string())
        );
    }

    #[test]
    fn legacy_code_written_during_migration_is_preserved_until_cleanup() {
        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let config = Config::new_with_file_secrets(&config_path, &secrets_path).unwrap();
        let old_process = Config::new_with_file_secrets(&config_path, &secrets_path).unwrap();
        let first = StoredPendingCode {
            code: "FIRST-CODE".to_string(),
            gateway_type: "telegram".to_string(),
            expires_at: 101,
        };
        let second = StoredPendingCode {
            code: "SECOND-CODE".to_string(),
            gateway_type: "slack".to_string(),
            expires_at: 101,
        };
        config
            .set_param(PENDING_CODES_CONFIG_KEY, [&first])
            .unwrap();
        let stale_snapshot = config.get_param(PENDING_CODES_CONFIG_KEY).unwrap();
        PairingStore::revoke_legacy_pending_codes(&config, stale_snapshot).unwrap();

        old_process
            .set_param(PENDING_CODES_CONFIG_KEY, [&first, &second])
            .unwrap();

        assert!(PairingStore::migrate_pending_codes(&config)
            .unwrap_err()
            .to_string()
            .contains("stop older Goose processes"));
        let legacy_codes: Vec<StoredPendingCode> =
            config.get_param(PENDING_CODES_CONFIG_KEY).unwrap();
        assert_eq!(
            legacy_codes
                .iter()
                .map(|pending| pending.code.as_str())
                .collect::<Vec<_>>(),
            ["FIRST-CODE", "SECOND-CODE"]
        );

        config.delete(PENDING_CODES_CONFIG_KEY).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "FIRST-CODE", 100).unwrap(),
            None
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "SECOND-CODE", 100).unwrap(),
            None
        );
    }

    #[test]
    fn legacy_codes_consumed_by_old_writer_cannot_be_reused() {
        let directory = TempDir::new().unwrap();
        let config_path = directory.path().join("config.yaml");
        let secrets_path = directory.path().join("secrets.yaml");
        let config = Config::new_with_file_secrets(&config_path, &secrets_path).unwrap();
        let old_process = Config::new_with_file_secrets(&config_path, &secrets_path).unwrap();
        let first = StoredPendingCode {
            code: "FIRST-CODE".to_string(),
            gateway_type: "telegram".to_string(),
            expires_at: 101,
        };
        let second = StoredPendingCode {
            code: "SECOND-CODE".to_string(),
            gateway_type: "slack".to_string(),
            expires_at: 101,
        };
        old_process
            .set_param(PENDING_CODES_CONFIG_KEY, [&first, &second])
            .unwrap();

        assert!(PairingStore::migrate_pending_codes(&config).is_err());
        old_process
            .set_param(PENDING_CODES_CONFIG_KEY, [&second])
            .unwrap();
        old_process.delete(PENDING_CODES_CONFIG_KEY).unwrap();

        assert_eq!(
            PairingStore::consume_pending_code_in(&config, &first.code, 100).unwrap(),
            None
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, &second.code, 100).unwrap(),
            None
        );
    }

    #[test]
    fn imported_legacy_codes_already_in_secret_state_are_revoked() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let code = "IMPORTED-CODE";
        config
            .set_secret(
                PENDING_CODES_SECRET_KEY,
                &StoredPendingCodes {
                    codes: vec![StoredPendingCode {
                        code: code.to_string(),
                        gateway_type: "telegram".to_string(),
                        expires_at: 101,
                    }],
                    legacy_import_complete: true,
                    imported_legacy_codes: vec![code.to_string()],
                },
            )
            .unwrap();

        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            None
        );
        assert!(config
            .get_secret::<StoredPendingCodes>(PENDING_CODES_SECRET_KEY)
            .unwrap()
            .codes
            .is_empty());
    }

    #[test]
    fn pending_code_migration_accepts_legacy_secret_vec() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let code = "SECRET-VEC-CODE";
        let legacy_codes = vec![StoredPendingCode {
            code: code.to_string(),
            gateway_type: "telegram".to_string(),
            expires_at: 101,
        }];
        config
            .set_secret(PENDING_CODES_SECRET_KEY, &legacy_codes)
            .unwrap();

        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            Some("telegram".to_string())
        );
        assert!(
            config
                .get_secret::<StoredPendingCodes>(PENDING_CODES_SECRET_KEY)
                .unwrap()
                .legacy_import_complete
        );
        PairingStore::revoke_legacy_pending_codes(&config, legacy_codes).unwrap();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, code, 100).unwrap(),
            None
        );
    }

    #[test]
    fn migration_without_legacy_config_does_not_touch_secret_storage() {
        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let secrets_path = directory.path().join("secrets.yaml");

        assert!(!secrets_path.exists());
        PairingStore::migrate_pending_codes(&config).unwrap();
        assert!(!secrets_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unmatched_pending_codes_do_not_rewrite_secret_storage() {
        use std::os::unix::fs::MetadataExt;

        let directory = TempDir::new().unwrap();
        let config = test_config(&directory);
        let secrets_path = directory.path().join("secrets.yaml");
        PairingStore::store_pending_code_in(&config, "VALID-CODE", "telegram", 101).unwrap();
        PairingStore::store_pending_code_in(&config, "EXPIRED-CODE", "slack", 99).unwrap();

        let initial_inode = std::fs::metadata(&secrets_path).unwrap().ino();
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "ZZZZZZ", 100).unwrap(),
            None
        );
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "ZZZZZZ", 100).unwrap(),
            None
        );
        assert_eq!(
            std::fs::metadata(&secrets_path).unwrap().ino(),
            initial_inode
        );

        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "VALID-CODE", 100).unwrap(),
            Some("telegram".to_string())
        );
        let consumed_inode = std::fs::metadata(&secrets_path).unwrap().ino();
        assert_ne!(consumed_inode, initial_inode);
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "VALID-CODE", 100).unwrap(),
            None
        );
        assert_eq!(
            std::fs::metadata(&secrets_path).unwrap().ino(),
            consumed_inode
        );

        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "EXPIRED-CODE", 100).unwrap(),
            None
        );
        let expired_inode = std::fs::metadata(&secrets_path).unwrap().ino();
        assert_ne!(expired_inode, consumed_inode);
        assert_eq!(
            PairingStore::consume_pending_code_in(&config, "EXPIRED-CODE", 98).unwrap(),
            None
        );
        assert_eq!(
            std::fs::metadata(&secrets_path).unwrap().ino(),
            expired_inode
        );
    }
}
