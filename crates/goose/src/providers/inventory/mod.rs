pub mod registrations;
mod resolver;

pub use resolver::{
    default_inventory_identity_resolver, InventoryConfiguredResolver, InventoryIdentityResolver,
    InventoryRegistration, InventoryResolvers,
};

use super::base::{ConfigKey, ModelInfo, Provider, ProviderType};
use super::canonical::{map_provider_name, map_to_canonical_model, CanonicalModelRegistry};
use crate::config::declarative_providers::{DeclarativeProviderConfig, ProviderEngine};
use crate::config::Config;
use crate::session::session_manager::SessionStorage;
use crate::utils::bytes_to_hex;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::warn;

const STALE_AFTER_HOURS: i64 = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInventoryEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub description: String,
    pub default_model: String,
    pub configured: bool,
    pub available: bool,
    pub provider_type: ProviderType,
    pub acp: bool,
    pub visible_in_setup: bool,
    pub deprecated: bool,
    pub replacement: Option<String>,
    pub config_keys: Vec<ConfigKey>,
    pub setup_steps: Vec<String>,
    pub supports_refresh: bool,
    pub refreshing: bool,
    pub models: Vec<InventoryModel>,
    pub last_updated_at: Option<DateTime<Utc>>,
    pub last_refresh_attempt_at: Option<DateTime<Utc>>,
    pub last_refresh_error: Option<String>,
}

/// Families whose latest model should be surfaced in the compact picker.
/// Each entry is matched against the `family` field of enriched models.
const RECOMMENDED_FAMILIES: &[&str] = &[
    "claude-opus",
    "claude-sonnet",
    "gpt",
    "gpt-mini",
    "glm",
    "gemini-pro",
    "gemini-flash",
    "gemma",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryModel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// Whether this model should appear in the compact recommended picker.
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryIdentity {
    pub provider_id: String,
    pub provider_family: String,
    pub inventory_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct InventoryIdentityInput {
    pub provider_id: String,
    pub provider_family: String,
    pub public_inputs: BTreeMap<String, String>,
    pub secret_inputs: BTreeMap<String, String>,
}

impl InventoryIdentityInput {
    pub fn new(
        provider_id: impl Into<String>,
        provider_family: impl Into<String>,
    ) -> InventoryIdentityInput {
        InventoryIdentityInput {
            provider_id: provider_id.into(),
            provider_family: provider_family.into(),
            public_inputs: BTreeMap::new(),
            secret_inputs: BTreeMap::new(),
        }
    }

    pub fn with_public(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> InventoryIdentityInput {
        self.public_inputs.insert(key.into(), value.into());
        self
    }

    pub fn with_secret(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> InventoryIdentityInput {
        self.secret_inputs.insert(key.into(), value.into());
        self
    }

    pub fn into_identity(self) -> Result<InventoryIdentity> {
        let InventoryIdentityInput {
            provider_id,
            provider_family,
            public_inputs,
            secret_inputs,
        } = self;
        let payload = serde_json::json!({
            "provider_family": provider_family,
            "public_inputs": public_inputs,
            "secret_inputs": secret_inputs,
        });
        let digest = Sha256::digest(serde_json::to_vec(&payload)?);
        Ok(InventoryIdentity {
            provider_id,
            provider_family,
            inventory_key: bytes_to_hex(digest),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshSkipReason {
    UnknownProvider,
    NotConfigured,
    DoesNotSupportRefresh,
    AlreadyRefreshing,
}

#[derive(Debug, Clone)]
pub struct RefreshSkip {
    pub provider_id: String,
    pub reason: RefreshSkipReason,
}

#[derive(Debug, Clone)]
pub(crate) struct RefreshJob {
    pub provider_id: String,
    pub identity: InventoryIdentity,
    pub toolshim: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RefreshPlan {
    pub started: Vec<String>,
    pub skipped: Vec<RefreshSkip>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RefreshJobPlan {
    pub started: Vec<RefreshJob>,
    pub skipped: Vec<RefreshSkip>,
}

impl RefreshJobPlan {
    pub(crate) fn into_public_plan(self) -> RefreshPlan {
        RefreshPlan {
            started: self
                .started
                .into_iter()
                .map(|job| job.provider_id)
                .collect(),
            skipped: self.skipped,
        }
    }
}

#[derive(Clone)]
pub struct ProviderInventoryService {
    storage: Arc<SessionStorage>,
    refreshing_keys: Arc<RwLock<HashSet<String>>>,
}

pub(crate) struct RefreshGuard {
    inventory_key: String,
    refreshing_keys: Arc<RwLock<HashSet<String>>>,
    completed: bool,
}

impl RefreshGuard {
    /// Mark the refresh as finished and remove its inventory key from the
    /// refreshing-keys set. `RefreshGuard` is the single owner of refresh-key
    /// removal; store methods do not clear keys themselves.
    pub fn complete(&mut self) {
        if self.completed {
            return;
        }
        let mut refreshing_keys = self
            .refreshing_keys
            .write()
            .unwrap_or_else(|poisoned| recover_poisoned_write(poisoned, "refreshing_keys"));
        refreshing_keys.remove(&self.inventory_key);
        self.completed = true;
    }
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        self.complete();
    }
}

fn recover_poisoned_read<'a, T>(
    poisoned: PoisonError<RwLockReadGuard<'a, T>>,
    lock_name: &str,
) -> RwLockReadGuard<'a, T> {
    warn!(
        lock = lock_name,
        "recovering poisoned provider inventory read lock"
    );
    poisoned.into_inner()
}

fn recover_poisoned_write<'a, T>(
    poisoned: PoisonError<RwLockWriteGuard<'a, T>>,
    lock_name: &str,
) -> RwLockWriteGuard<'a, T> {
    warn!(
        lock = lock_name,
        "recovering poisoned provider inventory write lock"
    );
    poisoned.into_inner()
}

#[derive(Debug, Clone)]
struct InventorySnapshot {
    models: Vec<InventoryModel>,
    last_updated_at: Option<DateTime<Utc>>,
    last_refresh_attempt_at: Option<DateTime<Utc>>,
    last_refresh_error: Option<String>,
}

#[derive(Debug, Clone)]
struct ProviderDescriptor {
    provider_id: String,
    provider_name: String,
    description: String,
    default_model: String,
    identity: InventoryIdentity,
    configured: bool,
    available: bool,
    provider_type: ProviderType,
    acp: bool,
    visible_in_setup: bool,
    deprecated: bool,
    replacement: Option<String>,
    config_keys: Vec<ConfigKey>,
    setup_steps: Vec<String>,
    supports_refresh: bool,
    toolshim: bool,
    static_models: Vec<ModelInfo>,
}

impl ProviderInventoryService {
    pub fn new(storage: Arc<SessionStorage>) -> ProviderInventoryService {
        ProviderInventoryService {
            storage,
            refreshing_keys: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub async fn entry_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<Option<ProviderInventoryEntry>> {
        let Some(descriptor) = self.describe_provider(provider_id).await? else {
            return Ok(None);
        };
        let snapshot = self.read_snapshot(&descriptor.identity).await?;
        let refreshing = self
            .refreshing_keys
            .read()
            .unwrap_or_else(|poisoned| recover_poisoned_read(poisoned, "refreshing_keys"))
            .contains(&descriptor.identity.inventory_key);
        let models = inventory_models_from_snapshot(
            snapshot.as_ref(),
            &descriptor.identity.provider_family,
            &descriptor.static_models,
            descriptor.supports_refresh,
        );

        Ok(Some(ProviderInventoryEntry {
            provider_id: descriptor.provider_id,
            provider_name: descriptor.provider_name,
            description: descriptor.description,
            default_model: descriptor.default_model,
            configured: descriptor.configured,
            available: descriptor.available,
            provider_type: descriptor.provider_type,
            acp: descriptor.acp,
            visible_in_setup: descriptor.visible_in_setup,
            deprecated: descriptor.deprecated,
            replacement: descriptor.replacement,
            config_keys: descriptor.config_keys,
            setup_steps: descriptor.setup_steps,
            supports_refresh: descriptor.supports_refresh,
            refreshing,
            models,
            last_updated_at: snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.last_updated_at),
            last_refresh_attempt_at: snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.last_refresh_attempt_at),
            last_refresh_error: snapshot.and_then(|snapshot| snapshot.last_refresh_error),
        }))
    }

    pub async fn find_entry_for_provider(
        &self,
        provider_id: &str,
    ) -> Option<ProviderInventoryEntry> {
        match self.entry_for_provider(provider_id).await {
            Ok(entry) => entry,
            Err(error) => {
                warn!(
                    provider = %provider_id,
                    %error,
                    "failed to look up provider inventory entry"
                );
                None
            }
        }
    }

    pub async fn entries(&self, provider_ids: &[String]) -> Result<Vec<ProviderInventoryEntry>> {
        let ids = self.resolve_provider_ids(provider_ids).await;
        let handles: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let this = self.clone();
                tokio::spawn(async move { this.entry_for_provider(&id).await })
            })
            .collect();
        let results = futures::future::join_all(handles).await;
        let mut entries = Vec::with_capacity(results.len());
        for result in results {
            let inner = result.context("provider inventory task panicked")?;
            if let Some(entry) = inner? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    pub async fn plan_refresh(&self, provider_ids: &[String]) -> Result<RefreshPlan> {
        self.plan_refresh_jobs(provider_ids)
            .await
            .map(RefreshJobPlan::into_public_plan)
    }

    pub(crate) async fn plan_refresh_jobs(
        &self,
        provider_ids: &[String],
    ) -> Result<RefreshJobPlan> {
        let ids = self.resolve_provider_ids(provider_ids).await;
        let mut plan = RefreshJobPlan::default();
        let mut inserted_refreshing = Vec::new();

        for provider_id in ids {
            let Some(descriptor) = self.describe_provider(&provider_id).await? else {
                plan.skipped.push(RefreshSkip {
                    provider_id,
                    reason: RefreshSkipReason::UnknownProvider,
                });
                continue;
            };

            if !descriptor.supports_refresh {
                plan.skipped.push(RefreshSkip {
                    provider_id: descriptor.provider_id,
                    reason: RefreshSkipReason::DoesNotSupportRefresh,
                });
                continue;
            }

            if !descriptor.configured {
                plan.skipped.push(RefreshSkip {
                    provider_id: descriptor.provider_id,
                    reason: RefreshSkipReason::NotConfigured,
                });
                continue;
            }

            let already_refreshing = {
                let mut refreshing_keys = self
                    .refreshing_keys
                    .write()
                    .unwrap_or_else(|poisoned| recover_poisoned_write(poisoned, "refreshing_keys"));
                if refreshing_keys.contains(&descriptor.identity.inventory_key) {
                    true
                } else {
                    refreshing_keys.insert(descriptor.identity.inventory_key.clone());
                    false
                }
            };

            if already_refreshing {
                plan.skipped.push(RefreshSkip {
                    provider_id: descriptor.provider_id,
                    reason: RefreshSkipReason::AlreadyRefreshing,
                });
                continue;
            }

            inserted_refreshing.push(descriptor.identity.clone());
            if let Err(error) = self.mark_refresh_started(&descriptor.identity).await {
                self.clear_refreshing_many(&inserted_refreshing);
                return Err(error);
            }

            plan.started.push(RefreshJob {
                provider_id: descriptor.provider_id,
                identity: descriptor.identity,
                toolshim: descriptor.toolshim,
            });
        }

        Ok(plan)
    }

    pub async fn store_refreshed_models(
        &self,
        provider_id: &str,
        model_ids: &[String],
    ) -> Result<()> {
        let descriptor = self.require_provider(provider_id).await?;
        self.store_refreshed_models_for_identity(&descriptor.identity, model_ids)
            .await?;
        self.clear_refreshing_many(std::slice::from_ref(&descriptor.identity));
        Ok(())
    }

    pub(crate) async fn store_refreshed_models_for_identity(
        &self,
        identity: &InventoryIdentity,
        model_ids: &[String],
    ) -> Result<()> {
        let models = enrich_model_ids_with_canonical(&identity.provider_family, model_ids);
        let now = Utc::now();
        let pool = self.storage.pool().await?;
        let mut tx = pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO provider_inventory_entries (
                inventory_key,
                provider_id,
                provider_family,
                last_updated_at,
                last_refresh_attempt_at,
                last_refresh_error,
                updated_at
            ) VALUES (?, ?, ?, ?, ?, NULL, CURRENT_TIMESTAMP)
            ON CONFLICT(inventory_key) DO UPDATE SET
                provider_id = excluded.provider_id,
                provider_family = excluded.provider_family,
                last_updated_at = excluded.last_updated_at,
                last_refresh_attempt_at = excluded.last_refresh_attempt_at,
                last_refresh_error = NULL,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&identity.inventory_key)
        .bind(&identity.provider_id)
        .bind(&identity.provider_family)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM provider_inventory_models WHERE inventory_key = ?")
            .bind(&identity.inventory_key)
            .execute(&mut *tx)
            .await?;

        for (ordinal, model) in models.iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO provider_inventory_models (
                    inventory_key,
                    ordinal,
                    model_id,
                    name,
                    family,
                    context_limit,
                    reasoning,
                    recommended
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&identity.inventory_key)
            .bind(i64::try_from(ordinal)?)
            .bind(&model.id)
            .bind(&model.name)
            .bind(&model.family)
            .bind(model.context_limit.map(i64::try_from).transpose()?)
            .bind(model.reasoning)
            .bind(model.recommended)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn store_refresh_error(
        &self,
        provider_id: &str,
        error: impl Into<String>,
    ) -> Result<()> {
        let descriptor = self.require_provider(provider_id).await?;
        self.store_refresh_error_for_identity(&descriptor.identity, error)
            .await?;
        self.clear_refreshing_many(std::slice::from_ref(&descriptor.identity));
        Ok(())
    }

    pub(crate) async fn store_refresh_error_for_identity(
        &self,
        identity: &InventoryIdentity,
        error: impl Into<String>,
    ) -> Result<()> {
        let error = error.into();
        let existing = self.read_snapshot(identity).await?;

        sqlx::query(
            r#"
            INSERT INTO provider_inventory_entries (
                inventory_key,
                provider_id,
                provider_family,
                last_updated_at,
                last_refresh_attempt_at,
                last_refresh_error,
                updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(inventory_key) DO UPDATE SET
                provider_id = excluded.provider_id,
                provider_family = excluded.provider_family,
                last_updated_at = excluded.last_updated_at,
                last_refresh_attempt_at = excluded.last_refresh_attempt_at,
                last_refresh_error = excluded.last_refresh_error,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&identity.inventory_key)
        .bind(&identity.provider_id)
        .bind(&identity.provider_family)
        .bind(existing.and_then(|snapshot| snapshot.last_updated_at.map(|time| time.to_rfc3339())))
        .bind(Utc::now().to_rfc3339())
        .bind(error)
        .execute(self.storage.pool().await?)
        .await?;

        Ok(())
    }

    fn clear_refreshing_many(&self, identities: &[InventoryIdentity]) {
        let mut refreshing_keys = self
            .refreshing_keys
            .write()
            .unwrap_or_else(|poisoned| recover_poisoned_write(poisoned, "refreshing_keys"));
        for identity in identities {
            refreshing_keys.remove(&identity.inventory_key);
        }
    }

    pub(crate) fn refresh_guard(&self, identity: &InventoryIdentity) -> RefreshGuard {
        RefreshGuard {
            inventory_key: identity.inventory_key.clone(),
            refreshing_keys: Arc::clone(&self.refreshing_keys),
            completed: false,
        }
    }

    pub(crate) async fn refresh_with_provider(
        &self,
        provider_name: &str,
        provider: &Arc<dyn Provider>,
        inventory: &mut ProviderInventoryEntry,
        context: &str,
    ) {
        let provider_id = provider_name.to_string();
        match self
            .plan_refresh_jobs(std::slice::from_ref(&provider_id))
            .await
        {
            Ok(plan)
                if plan
                    .started
                    .iter()
                    .any(|job| job.provider_id == provider_id) =>
            {
                let refresh_job = plan
                    .started
                    .into_iter()
                    .find(|job| job.provider_id == provider_id);
                if let Some(refresh_job) = refresh_job {
                    let mut refresh_guard = self.refresh_guard(&refresh_job.identity);
                    let toolshim = refresh_job.toolshim;
                    let fetch_result: Result<Vec<String>> =
                        match ensure_refresh_identity_current(&provider_id, &refresh_job.identity)
                            .await
                        {
                            Ok(()) => {
                                match AssertUnwindSafe(provider.fetch_recommended_models(toolshim))
                                    .catch_unwind()
                                    .await
                                {
                                    Ok(Ok(models)) => Ok(models),
                                    Ok(Err(error)) => Err(anyhow::anyhow!(error.to_string())),
                                    Err(_) => Err(anyhow::anyhow!(
                                        "provider inventory refresh task panicked"
                                    )),
                                }
                            }
                            Err(error) => Err(error),
                        };
                    match fetch_result {
                        Ok(models) => {
                            if let Err(error) = self
                                .store_refreshed_models_for_identity(&refresh_job.identity, &models)
                                .await
                            {
                                warn!(
                                    provider = %provider_id,
                                    context = %context,
                                    error = %error,
                                    "failed to store refreshed provider inventory"
                                );
                            } else {
                                refresh_guard.complete();
                            }
                        }
                        Err(error) => {
                            let error_message = error.to_string();
                            if let Err(store_error) = self
                                .store_refresh_error_for_identity(
                                    &refresh_job.identity,
                                    error_message.clone(),
                                )
                                .await
                            {
                                warn!(
                                    provider = %provider_id,
                                    context = %context,
                                    error = %store_error,
                                    "failed to store provider inventory refresh error"
                                );
                            } else {
                                refresh_guard.complete();
                            }
                            warn!(
                                provider = %provider_id,
                                context = %context,
                                error = %error_message,
                                "provider inventory refresh failed"
                            );
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(error) => warn!(
                provider = %provider_id,
                context = %context,
                error = %error,
                "failed to plan provider inventory refresh"
            ),
        }

        if let Some(refreshed_inventory) = self.find_entry_for_provider(provider_name).await {
            *inventory = refreshed_inventory;
        }
    }

    pub fn is_stale(entry: &ProviderInventoryEntry) -> bool {
        let Some(last_updated_at) = entry.last_updated_at else {
            return false;
        };
        entry.supports_refresh && Utc::now() - last_updated_at > Duration::hours(STALE_AFTER_HOURS)
    }

    async fn describe_provider(&self, provider_id: &str) -> Result<Option<ProviderDescriptor>> {
        let entry = match crate::providers::get_from_registry(provider_id).await {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };
        let metadata = entry.metadata().clone();
        let identity = crate::providers::inventory_identity(provider_id)
            .await
            .unwrap_or_else(|_| fallback_inventory_identity(provider_id))
            .into_identity()?;

        Ok(Some(ProviderDescriptor {
            provider_id: metadata.name.clone(),
            provider_name: metadata.display_name.clone(),
            description: metadata.description.clone(),
            default_model: metadata.default_model.clone(),
            identity,
            configured: if metadata.setup.as_ref().is_some_and(|setup| setup.acp) {
                crate::config::get_provider_entry(Config::global(), provider_id)
                    .is_some_and(|entry| entry.enabled && entry.configured)
            } else {
                entry.inventory_configured()
            },
            available: entry.inventory_configured(),
            provider_type: entry.provider_type(),
            acp: metadata.setup.as_ref().is_some_and(|setup| setup.acp),
            visible_in_setup: metadata.deprecated.is_none(),
            deprecated: metadata.deprecated.is_some(),
            replacement: metadata
                .deprecated
                .as_ref()
                .and_then(|deprecated| deprecated.replacement.clone()),
            config_keys: metadata.config_keys.clone(),
            setup_steps: metadata.setup_steps.clone(),
            supports_refresh: entry.supports_inventory_refresh(),
            toolshim: entry.toolshim_enabled(crate::model_config::global_toolshim()),
            static_models: metadata.known_models,
        }))
    }

    async fn require_provider(&self, provider_id: &str) -> Result<ProviderDescriptor> {
        self.describe_provider(provider_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Unknown provider: {}", provider_id))
    }

    async fn mark_refresh_started(&self, identity: &InventoryIdentity) -> Result<()> {
        let existing = self.read_snapshot(identity).await?;

        sqlx::query(
            r#"
            INSERT INTO provider_inventory_entries (
                inventory_key,
                provider_id,
                provider_family,
                last_updated_at,
                last_refresh_attempt_at,
                last_refresh_error,
                updated_at
            ) VALUES (?, ?, ?, ?, ?, NULL, CURRENT_TIMESTAMP)
            ON CONFLICT(inventory_key) DO UPDATE SET
                provider_id = excluded.provider_id,
                provider_family = excluded.provider_family,
                last_updated_at = excluded.last_updated_at,
                last_refresh_attempt_at = excluded.last_refresh_attempt_at,
                last_refresh_error = NULL,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&identity.inventory_key)
        .bind(&identity.provider_id)
        .bind(&identity.provider_family)
        .bind(existing.and_then(|snapshot| snapshot.last_updated_at.map(|time| time.to_rfc3339())))
        .bind(Utc::now().to_rfc3339())
        .execute(self.storage.pool().await?)
        .await?;

        Ok(())
    }

    async fn read_snapshot(
        &self,
        identity: &InventoryIdentity,
    ) -> Result<Option<InventorySnapshot>> {
        let pool = self.storage.pool().await?;
        let entry = sqlx::query(
            r#"
            SELECT last_updated_at, last_refresh_attempt_at, last_refresh_error
            FROM provider_inventory_entries
            WHERE inventory_key = ?
            "#,
        )
        .bind(&identity.inventory_key)
        .fetch_optional(pool)
        .await?;

        let Some(entry) = entry else {
            return Ok(None);
        };

        let last_updated_at = parse_optional_datetime(entry.try_get("last_updated_at")?)?;
        let last_refresh_attempt_at =
            parse_optional_datetime(entry.try_get("last_refresh_attempt_at")?)?;
        let last_refresh_error = entry.try_get("last_refresh_error")?;

        let rows = sqlx::query(
            r#"
            SELECT model_id, name, family, context_limit, reasoning, recommended
            FROM provider_inventory_models
            WHERE inventory_key = ?
            ORDER BY ordinal
            "#,
        )
        .bind(&identity.inventory_key)
        .fetch_all(pool)
        .await?;

        let models = rows
            .into_iter()
            .map(|row| {
                Ok(InventoryModel {
                    id: row.try_get("model_id")?,
                    name: row.try_get("name")?,
                    family: row.try_get("family")?,
                    context_limit: row
                        .try_get::<Option<i64>, _>("context_limit")?
                        .map(usize::try_from)
                        .transpose()?,
                    reasoning: row.try_get("reasoning")?,
                    recommended: row
                        .try_get::<Option<bool>, _>("recommended")?
                        .unwrap_or(false),
                })
            })
            .collect::<Result<Vec<_>, anyhow::Error>>()?;

        Ok(Some(InventorySnapshot {
            models,
            last_updated_at,
            last_refresh_attempt_at,
            last_refresh_error,
        }))
    }

    async fn resolve_provider_ids(&self, provider_ids: &[String]) -> Vec<String> {
        let mut ids = if provider_ids.is_empty() {
            crate::providers::providers()
                .await
                .into_iter()
                .map(|(metadata, _)| metadata.name)
                .collect::<Vec<_>>()
        } else {
            provider_ids.to_vec()
        };
        ids.sort();
        ids.dedup();
        ids
    }
}

pub(crate) async fn ensure_refresh_identity_current(
    provider_id: &str,
    planned_identity: &InventoryIdentity,
) -> Result<()> {
    let current_identity = crate::providers::inventory_identity(provider_id)
        .await?
        .into_identity()?;
    if current_identity != *planned_identity {
        anyhow::bail!("provider inventory identity changed before refresh completed");
    }
    Ok(())
}

pub fn default_inventory_identity(
    provider_id: &str,
    provider_family: &str,
    config_keys: &[ConfigKey],
    config: &Config,
) -> InventoryIdentityInput {
    let mut identity = InventoryIdentityInput::new(provider_id, provider_family);

    for key in config_keys {
        if key.secret {
            if let Some(value) = config_secret_value(config, &key.name) {
                identity.secret_inputs.insert(key.name.clone(), value);
            }
        } else if let Some(value) = config_param_value(config, &key.name) {
            identity.public_inputs.insert(key.name.clone(), value);
        }
    }

    identity
}

pub fn default_inventory_configured(config_keys: &[ConfigKey], config: &Config) -> bool {
    config_keys.iter().all(|key| {
        if !key.required {
            return true;
        }
        if key.default.is_some() {
            return true;
        }
        if key.secret {
            config.get_secret::<serde_json::Value>(&key.name).is_ok()
        } else {
            config.get_param::<serde_json::Value>(&key.name).is_ok()
        }
    })
}

pub fn declarative_inventory_identity(
    config: &DeclarativeProviderConfig,
) -> Result<InventoryIdentityInput> {
    let global = Config::global();
    let mut identity = InventoryIdentityInput::new(
        config.name.clone(),
        config
            .catalog_provider_id
            .clone()
            .unwrap_or_else(|| match config.engine {
                ProviderEngine::OpenAI => "openai".to_string(),
                ProviderEngine::Anthropic => "anthropic".to_string(),
                ProviderEngine::Ollama => "ollama".to_string(),
            }),
    );

    identity
        .public_inputs
        .insert("base_url".to_string(), config.base_url.clone());

    if let Some(base_path) = &config.base_path {
        identity
            .public_inputs
            .insert("base_path".to_string(), base_path.clone());
    }
    if let Some(catalog_provider_id) = &config.catalog_provider_id {
        identity.public_inputs.insert(
            "catalog_provider_id".to_string(),
            catalog_provider_id.clone(),
        );
    }
    if let Some(dynamic_models) = config.dynamic_models {
        identity
            .public_inputs
            .insert("dynamic_models".to_string(), dynamic_models.to_string());
    }
    identity.public_inputs.insert(
        "skip_canonical_filtering".to_string(),
        config.skip_canonical_filtering.to_string(),
    );
    identity.public_inputs.insert(
        "toolshim".to_string(),
        (config.toolshim || crate::model_config::global_toolshim()).to_string(),
    );
    if !config.models.is_empty() {
        identity.public_inputs.insert(
            "models".to_string(),
            serde_json::to_string(
                &config
                    .models
                    .iter()
                    .map(|model| &model.name)
                    .collect::<Vec<_>>(),
            )?,
        );
    }
    if let Some(headers) = &config.headers {
        identity
            .public_inputs
            .insert("headers".to_string(), serialize_string_map(headers)?);
    }
    if let Some(header_name) = &config.session_id_header_override {
        identity.public_inputs.insert(
            "session_id_header_override".to_string(),
            header_name.clone(),
        );
    }
    if !config.api_key_env.is_empty() {
        if let Some(value) = config_secret_value(global, &config.api_key_env) {
            identity
                .secret_inputs
                .insert(config.api_key_env.clone(), value);
        }
    }
    if let Some(auth) = &config.auth {
        identity
            .secret_inputs
            .insert("auth_command".to_string(), auth.command.clone());
        identity
            .secret_inputs
            .insert("auth_args".to_string(), serde_json::to_string(&auth.args)?);
        if let Some(cwd) = &auth.cwd {
            identity
                .secret_inputs
                .insert("auth_cwd".to_string(), cwd.clone());
        }
    }

    Ok(identity)
}

pub fn config_param_value(config: &Config, key: &str) -> Option<String> {
    config
        .get_param::<serde_json::Value>(key)
        .ok()
        .and_then(|value| normalize_json_value(&value))
}

pub fn config_secret_value(config: &Config, key: &str) -> Option<String> {
    config
        .get_secret::<serde_json::Value>(key)
        .ok()
        .and_then(|value| normalize_json_value(&value))
}

pub fn serialize_string_map(map: &HashMap<String, String>) -> Result<String> {
    let ordered = map
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    Ok(serde_json::to_string(&ordered)?)
}

fn parse_optional_datetime(value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| value.parse::<DateTime<Utc>>())
        .transpose()
        .map_err(Into::into)
}

fn normalize_json_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) if value.is_empty() => None,
        serde_json::Value::String(value) => Some(value.clone()),
        other => serde_json::to_string(other).ok(),
    }
}

fn fallback_inventory_identity(provider_id: &str) -> InventoryIdentityInput {
    InventoryIdentityInput::new(
        provider_id.to_string(),
        map_provider_name(provider_id).to_string(),
    )
}

fn is_databricks_v2_model_service(provider_family: &str, model_id: &str) -> bool {
    provider_family == "databricks_v2" && model_id.splitn(3, '.').count() == 3
}

fn enrich_model_ids_with_canonical(
    provider_family: &str,
    model_ids: &[String],
) -> Vec<InventoryModel> {
    if provider_family == "litellm" {
        return model_ids
            .iter()
            .map(|id| InventoryModel {
                id: id.clone(),
                name: id.clone(),
                family: None,
                context_limit: None,
                reasoning: None,
                recommended: false,
            })
            .collect();
    }

    let mut models: Vec<InventoryModel> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();

    for id in model_ids {
        let model = enriched_model(provider_family, id, None);
        let dedup_key = if is_databricks_v2_model_service(provider_family, id) {
            id
        } else {
            &model.name
        };
        if !seen_keys.insert(dedup_key.clone()) {
            continue;
        }
        models.push(model);
    }

    // For Databricks providers, prefer goose- prefixed model_ids when there are duplicates.
    // Re-scan: if a later model_id with "goose-" prefix maps to the same display name,
    // swap it in.
    if matches!(provider_family, "databricks" | "databricks_v2") {
        let mut name_to_idx: HashMap<String, usize> = HashMap::new();
        for (idx, model) in models.iter().enumerate() {
            if !is_databricks_v2_model_service(provider_family, &model.id) {
                name_to_idx.insert(model.name.clone(), idx);
            }
        }
        for id in model_ids {
            if !id.starts_with("goose-") {
                continue;
            }
            let candidate = enriched_model(provider_family, id, None);
            if let Some(&idx) = name_to_idx.get(&candidate.name) {
                if !models[idx].id.starts_with("goose-") {
                    models[idx].id = candidate.id;
                }
            }
        }
    }

    // Mark the latest model per recommended family.
    let mut seen_recommended_families: HashSet<String> = HashSet::new();
    for model in &mut models {
        if let Some(family) = &model.family {
            if RECOMMENDED_FAMILIES.contains(&family.as_str())
                && seen_recommended_families.insert(family.clone())
            {
                model.recommended = true;
            }
        }
    }

    models
}

fn configured_models_to_inventory(
    provider_family: &str,
    models: &[ModelInfo],
) -> Vec<InventoryModel> {
    let mut result: Vec<InventoryModel> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for model in models {
        let enriched = enriched_model(provider_family, &model.name, model.context_limit);
        if seen_names.insert(enriched.name.clone()) {
            result.push(enriched);
        }
    }

    let mut seen_recommended_families: HashSet<String> = HashSet::new();
    for model in &mut result {
        if let Some(family) = &model.family {
            if RECOMMENDED_FAMILIES.contains(&family.as_str())
                && seen_recommended_families.insert(family.clone())
            {
                model.recommended = true;
            }
        }
    }

    result
}

fn inventory_models_from_snapshot(
    snapshot: Option<&InventorySnapshot>,
    provider_family: &str,
    configured_models: &[ModelInfo],
    supports_refresh: bool,
) -> Vec<InventoryModel> {
    if !supports_refresh {
        return configured_models_to_inventory(provider_family, configured_models);
    }

    match snapshot {
        Some(snapshot) if !snapshot.models.is_empty() || snapshot.last_updated_at.is_some() => {
            snapshot.models.clone()
        }
        _ => configured_models_to_inventory(provider_family, configured_models),
    }
}

fn enriched_model(
    provider_family: &str,
    model_id: &str,
    fallback_context_limit: Option<usize>,
) -> InventoryModel {
    let registry = CanonicalModelRegistry::bundled().ok();
    let canonical = registry.as_ref().and_then(|registry| {
        let canonical_id = map_to_canonical_model(provider_family, model_id, registry)?;
        let (provider, model) = canonical_id.split_once('/')?;
        registry.get(provider, model).cloned()
    });

    InventoryModel {
        id: model_id.to_string(),
        name: canonical
            .as_ref()
            .map(|model| model.name.clone())
            .unwrap_or_else(|| model_id.to_string()),
        family: canonical.as_ref().and_then(|model| model.family.clone()),
        context_limit: canonical
            .as_ref()
            .map(|model| model.limit.context)
            .or(fallback_context_limit),
        reasoning: canonical.as_ref().and_then(|model| model.reasoning),
        recommended: false,
    }
}

pub async fn create_tables(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS provider_inventory_entries (
            inventory_key TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            provider_family TEXT NOT NULL,
            last_updated_at TEXT,
            last_refresh_attempt_at TEXT,
            last_refresh_error TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS provider_inventory_models (
            inventory_key TEXT NOT NULL REFERENCES provider_inventory_entries(inventory_key) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL,
            model_id TEXT NOT NULL,
            name TEXT NOT NULL,
            family TEXT,
            context_limit INTEGER,
            reasoning BOOLEAN,
            recommended BOOLEAN,
            PRIMARY KEY (inventory_key, ordinal)
        )
        "#,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_provider_inventory_provider_id ON provider_inventory_entries(provider_id)",
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity(provider_id: &str, inventory_key: &str) -> InventoryIdentity {
        InventoryIdentity {
            provider_id: provider_id.to_string(),
            provider_family: provider_id.to_string(),
            inventory_key: inventory_key.to_string(),
        }
    }

    #[test]
    fn refresh_guard_complete_clears_refreshing_key() {
        let refreshing_keys = Arc::new(RwLock::new(HashSet::from(["key-a".to_string()])));
        let mut guard = RefreshGuard {
            inventory_key: "key-a".to_string(),
            refreshing_keys: Arc::clone(&refreshing_keys),
            completed: false,
        };

        guard.complete();
        guard.complete();

        assert!(!refreshing_keys.read().unwrap().contains("key-a"));
    }

    #[tokio::test]
    async fn clear_refreshing_many_removes_all_inserted_keys() {
        let service =
            ProviderInventoryService::new(Arc::new(SessionStorage::new(std::env::temp_dir())));
        let left = test_identity("openai", "key-a");
        let right = test_identity("anthropic", "key-b");
        {
            let mut refreshing_keys = service.refreshing_keys.write().unwrap();
            refreshing_keys.insert(left.inventory_key.clone());
            refreshing_keys.insert(right.inventory_key.clone());
        }

        service.clear_refreshing_many(&[left, right]);

        assert!(service.refreshing_keys.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn identity_store_writes_to_captured_inventory_key() {
        let temp_dir = tempfile::tempdir().unwrap();
        let service = ProviderInventoryService::new(Arc::new(SessionStorage::new(
            temp_dir.path().to_path_buf(),
        )));
        let plan_time_identity = test_identity("openai", "plan-time-key");
        let current_identity = test_identity("openai", "current-key");
        let sentinel_model = "stark-plan-time-model".to_string();

        service
            .store_refreshed_models_for_identity(
                &plan_time_identity,
                std::slice::from_ref(&sentinel_model),
            )
            .await
            .unwrap();

        let plan_time_snapshot = service
            .read_snapshot(&plan_time_identity)
            .await
            .unwrap()
            .unwrap();
        assert!(plan_time_snapshot
            .models
            .iter()
            .any(|model| model.id == sentinel_model));
        assert!(service
            .read_snapshot(&current_identity)
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn inventory_identity_hash_changes_with_secret_inputs() {
        let left = InventoryIdentityInput::new("openai", "openai")
            .with_public("host", "https://api.openai.com")
            .with_secret("api_key", "secret-a")
            .into_identity()
            .unwrap();
        let right = InventoryIdentityInput::new("openai", "openai")
            .with_public("host", "https://api.openai.com")
            .with_secret("api_key", "secret-b")
            .into_identity()
            .unwrap();

        assert_ne!(left.inventory_key, right.inventory_key);
    }

    #[test]
    fn configured_models_use_canonical_enrichment() {
        let models = configured_models_to_inventory(
            "anthropic",
            &[ModelInfo::new("claude-sonnet-4-5").with_context_limit(0)],
        );

        assert_eq!(models.len(), 1);
        assert!(models[0].name.contains("Claude"));
    }

    #[test]
    fn databricks_v2_inventory_prefers_goose_model_ids_for_duplicate_names() {
        let models = enrich_model_ids_with_canonical(
            "databricks_v2",
            &[
                "databricks-gpt-5-5".to_string(),
                "goose-gpt-5-5".to_string(),
            ],
        );

        assert!(
            models.iter().any(|model| model.id == "goose-gpt-5-5"),
            "expected goose-gpt-5-5 to win duplicate canonical-name tie, got {models:?}"
        );
        assert!(
            !models.iter().any(|model| model.id == "databricks-gpt-5-5"),
            "expected databricks-gpt-5-5 to be replaced by goose-gpt-5-5, got {models:?}"
        );
    }

    #[test]
    fn databricks_v2_inventory_preserves_distinct_model_service_fqns() {
        let model_ids = [
            "alpha.prod.claude-sonnet-4-5",
            "beta.prod.claude-sonnet-4-5",
            "alpha.prod.claude-sonnet-4-5",
        ]
        .map(String::from);
        let models = enrich_model_ids_with_canonical("databricks_v2", &model_ids);

        let ids = models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "alpha.prod.claude-sonnet-4-5",
                "beta.prod.claude-sonnet-4-5"
            ]
        );
        assert_eq!(models[0].name, models[1].name);
    }

    #[test]
    fn inventory_uses_configured_models_before_first_successful_refresh() {
        let configured_models = [ModelInfo::new("claude-sonnet-4-5").with_context_limit(0)];
        let snapshot = InventorySnapshot {
            models: vec![],
            last_updated_at: None,
            last_refresh_attempt_at: Some(Utc::now()),
            last_refresh_error: Some("auth failed".to_string()),
        };

        let models =
            inventory_models_from_snapshot(Some(&snapshot), "anthropic", &configured_models, true);

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-sonnet-4-5");
    }

    #[test]
    fn inventory_preserves_empty_models_after_successful_refresh() {
        let configured_models = [ModelInfo::new("claude-sonnet-4-5").with_context_limit(0)];
        let snapshot = InventorySnapshot {
            models: vec![],
            last_updated_at: Some(Utc::now()),
            last_refresh_attempt_at: Some(Utc::now()),
            last_refresh_error: None,
        };

        let models =
            inventory_models_from_snapshot(Some(&snapshot), "anthropic", &configured_models, true);

        assert!(models.is_empty());
    }

    #[test]
    fn inventory_ignores_stale_snapshots_for_static_providers() {
        let configured_models = [ModelInfo::new("gpt-5.6").with_context_limit(0)];
        let snapshot = InventorySnapshot {
            models: vec![InventoryModel {
                id: "gpt-5.5".to_string(),
                name: "gpt-5.5".to_string(),
                family: None,
                context_limit: None,
                reasoning: None,
                recommended: false,
            }],
            last_updated_at: Some(Utc::now()),
            last_refresh_attempt_at: Some(Utc::now()),
            last_refresh_error: None,
        };

        let models = inventory_models_from_snapshot(
            Some(&snapshot),
            "chatgpt_codex",
            &configured_models,
            false,
        );

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6");
    }
}
