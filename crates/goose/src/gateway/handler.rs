use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agents::{Agent, AgentEvent, ExtensionConfig, SessionConfig};
use crate::config::extensions::get_enabled_extensions;
use crate::config::paths::Paths;
use crate::config::Config;
use crate::conversation::message::{ActionRequiredData, Message, MessageContent};
use crate::execution::manager::AgentManager;
use crate::permission::Permission;
use crate::session::SessionType;
use crate::session::{EnabledExtensionsState, ExtensionState, Session};

use super::pairing::PairingStore;
use super::{Gateway, GatewayConfig, IncomingMessage, OutgoingMessage, PairingState, PlatformUser};

/// Conservative default cap on tool-calling loops for gateway sessions.
///
/// Chat platforms like Telegram favor short, snappy replies, so the gateway
/// keeps a stricter default than the global `GOOSE_MAX_TURNS` ceiling.  Users
/// can override this through `GOOSE_GATEWAY_MAX_TURNS` (gateway-specific) or
/// `GOOSE_MAX_TURNS` (applies globally).
const DEFAULT_GATEWAY_MAX_TURNS: u32 = 5;

/// Resolve the max turns to use for a gateway session.
///
/// Precedence: `GOOSE_GATEWAY_MAX_TURNS` -> `GOOSE_MAX_TURNS` ->
/// `DEFAULT_GATEWAY_MAX_TURNS`.  Extracted as a pure function so the
/// precedence rules can be unit-tested without touching the global config.
fn resolve_gateway_max_turns(gateway_override: Option<u32>, global_max_turns: Option<u32>) -> u32 {
    gateway_override
        .or(global_max_turns)
        .unwrap_or(DEFAULT_GATEWAY_MAX_TURNS)
}

struct PendingConfirmation {
    agent: Arc<Agent>,
    session_id: String,
    request_ids: VecDeque<String>,
    cancel_token: CancellationToken,
}

type PerUserLocks = Arc<Mutex<HashMap<PlatformUser, Arc<Mutex<()>>>>>;

#[derive(Clone)]
pub struct GatewayHandler {
    agent_manager: Arc<AgentManager>,
    pairing_store: Arc<PairingStore>,
    gateway: Arc<dyn Gateway>,
    config: GatewayConfig,
    /// Tracks users who have a tool-confirmation prompt awaiting their reply.
    pending_confirmations: Arc<Mutex<HashMap<PlatformUser, PendingConfirmation>>>,
    /// Serializes `relay_to_session` per user; confirmation replies bypass this lock.
    turn_locks: PerUserLocks,
    /// Serializes confirmation replies without waiting for the active turn to finish.
    confirmation_reply_locks: PerUserLocks,
    /// When set, only platform user IDs in this list may enter the pairing flow.
    allowed_user_ids: Option<HashSet<String>>,
}

impl GatewayHandler {
    pub fn new(
        agent_manager: Arc<AgentManager>,
        pairing_store: Arc<PairingStore>,
        gateway: Arc<dyn Gateway>,
        config: GatewayConfig,
    ) -> Self {
        let allowed_user_ids = allowed_user_ids_from_config(&config);
        Self {
            agent_manager,
            pairing_store,
            gateway,
            config,
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            turn_locks: Arc::new(Mutex::new(HashMap::new())),
            confirmation_reply_locks: Arc::new(Mutex::new(HashMap::new())),
            allowed_user_ids,
        }
    }

    pub async fn deny_pending_confirmations(&self) {
        let pending: Vec<_> = self.pending_confirmations.lock().await.drain().collect();
        for (_, pending) in pending {
            let PendingConfirmation {
                agent,
                session_id,
                request_ids,
                cancel_token,
            } = pending;
            for request_id in request_ids {
                if let Err(error) = agent
                    .submit_tool_confirmation(&session_id, &request_id, Permission::DenyOnce)
                    .await
                {
                    tracing::error!(%error, %request_id, "failed to deny pending gateway confirmation");
                    cancel_token.cancel();
                }
            }
        }
    }

    async fn per_user_lock(locks: &PerUserLocks, user: &PlatformUser) -> Arc<Mutex<()>> {
        let mut locks = locks.lock().await;
        Arc::clone(
            locks
                .entry(user.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn prune_per_user_lock(locks: &PerUserLocks, user: &PlatformUser) {
        let mut locks = locks.lock().await;
        let in_use = locks
            .get(user)
            .is_some_and(|lock| Arc::strong_count(lock) > 1);
        if !in_use {
            locks.remove(user);
        }
    }

    pub async fn handle_message(&self, message: IncomingMessage) -> anyhow::Result<()> {
        if !is_user_allowed(&self.allowed_user_ids, &message.user) {
            tracing::info!(
                platform = %message.user.platform,
                user_id = %message.user.user_id,
                "gateway allowlist denied unlisted user"
            );
            return Ok(());
        }

        let pairing = self.pairing_store.get(&message.user).await?;

        match pairing {
            PairingState::Unpaired => {
                if let Some(gateway_type) = self.try_consume_code(message.text.trim()).await? {
                    if gateway_type == self.config.gateway_type {
                        self.complete_pairing(&message.user).await?;
                    } else {
                        self.gateway
                            .send_message(
                                &message.user,
                                OutgoingMessage::Text {
                                    body: "⚠️ That code is for a different gateway.".into(),
                                },
                            )
                            .await?;
                    }
                } else {
                    self.gateway
                        .send_message(
                            &message.user,
                            OutgoingMessage::Text {
                                body: "Welcome! Enter your pairing code to connect to goose."
                                    .into(),
                            },
                        )
                        .await?;
                }
            }
            PairingState::PendingCode { code, expires_at } => {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    self.pairing_store
                        .set(&message.user, PairingState::Unpaired)
                        .await?;
                    self.gateway
                        .send_message(
                            &message.user,
                            OutgoingMessage::Text {
                                body: "Your pairing code expired. Please request a new one.".into(),
                            },
                        )
                        .await?;
                } else if message.text.trim().eq_ignore_ascii_case(&code) {
                    self.complete_pairing(&message.user).await?;
                } else {
                    self.gateway
                        .send_message(
                            &message.user,
                            OutgoingMessage::Text {
                                body: "Invalid code. Please try again.".into(),
                            },
                        )
                        .await?;
                }
            }
            PairingState::Paired { session_id, .. } => {
                if self
                    .pending_confirmations
                    .lock()
                    .await
                    .contains_key(&message.user)
                {
                    self.handle_pending_confirmation(&message).await?;
                } else {
                    let turn_lock = Self::per_user_lock(&self.turn_locks, &message.user).await;
                    let turn_guard = turn_lock.lock().await;
                    let result = self.relay_to_session(&message, &session_id).await;
                    drop(turn_guard);
                    drop(turn_lock);
                    Self::prune_per_user_lock(&self.turn_locks, &message.user).await;
                    result?;
                }
            }
        }

        Ok(())
    }

    async fn handle_pending_confirmation(&self, message: &IncomingMessage) -> anyhow::Result<()> {
        let text = message.text.trim().to_lowercase();
        let permission = match text.as_str() {
            "approve" | "yes" | "y" => Some(Permission::AllowOnce),
            "approve always" => Some(Permission::AlwaysAllow),
            "deny" | "no" | "n" => Some(Permission::DenyOnce),
            "deny always" => Some(Permission::AlwaysDeny),
            _ => None,
        };

        let Some(permission) = permission else {
            self.gateway
                .send_message(
                    &message.user,
                    OutgoingMessage::Text {
                        body: "Please reply with: approve, approve always, deny, or deny always."
                            .into(),
                    },
                )
                .await?;
            return Ok(());
        };

        let reply_lock = Self::per_user_lock(&self.confirmation_reply_locks, &message.user).await;
        let reply_guard = reply_lock.lock().await;
        let result = self.submit_pending_confirmation(message, permission).await;
        drop(reply_guard);
        drop(reply_lock);
        Self::prune_per_user_lock(&self.confirmation_reply_locks, &message.user).await;
        result
    }

    async fn submit_pending_confirmation(
        &self,
        message: &IncomingMessage,
        permission: Permission,
    ) -> anyhow::Result<()> {
        let pending_confirmations = self.pending_confirmations.lock().await;
        let Some(pending) = pending_confirmations.get(&message.user) else {
            return Ok(());
        };
        let Some(request_id) = pending.request_ids.front().cloned() else {
            return Ok(());
        };
        let agent = pending.agent.clone();
        let session_id = pending.session_id.clone();
        let cancel_token = pending.cancel_token.clone();
        drop(pending_confirmations);
        if let Err(error) = agent
            .submit_tool_confirmation(&session_id, &request_id, permission)
            .await
        {
            cancel_token.cancel();
            self.pending_confirmations
                .lock()
                .await
                .remove(&message.user);
            return Err(error);
        }

        let mut pending_confirmations = self.pending_confirmations.lock().await;
        let remove_pending = pending_confirmations
            .get_mut(&message.user)
            .is_some_and(|pending| {
                if pending.request_ids.front() == Some(&request_id) {
                    pending.request_ids.pop_front();
                }
                pending.request_ids.is_empty()
            });
        if remove_pending {
            pending_confirmations.remove(&message.user);
        }

        Ok(())
    }

    async fn try_consume_code(&self, text: &str) -> anyhow::Result<Option<String>> {
        let normalized = text.to_uppercase().replace(['-', ' '], "");
        if normalized.len() == 6
            && normalized
                .chars()
                .all(|c| "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(c))
        {
            return self.pairing_store.consume_pending_code(&normalized).await;
        }
        Ok(None)
    }

    async fn complete_pairing(&self, user: &PlatformUser) -> anyhow::Result<()> {
        let working_dir = gateway_working_dir(&user.platform, &user.user_id);
        std::fs::create_dir_all(&working_dir)?;

        let session_name = format!(
            "{}/{}",
            user.platform,
            user.display_name.as_deref().unwrap_or(&user.user_id)
        );

        let config = Config::global();
        let session = self
            .agent_manager
            .session_manager()
            .create_session(
                working_dir,
                session_name,
                SessionType::Gateway,
                config.get_goose_mode().unwrap_or_default(),
            )
            .await?;

        let manager = self.agent_manager.session_manager();

        // Store the current provider and model config on the session so the agent
        // can be restored after LRU eviction, matching the start_agent flow.
        let mut update = manager.update(&session.id);
        let provider = config.get_goose_provider().ok();
        if let Some(ref provider) = provider {
            update = update.provider_name(provider);
        }
        if let (Some(ref provider), Ok(model_name)) = (&provider, config.get_goose_model()) {
            if let Ok(model_config) =
                crate::model_config::model_config_from_user_config(provider, &model_name)
            {
                update = update.model_config(model_config);
            }
        }

        // Store default extensions so load_extensions_from_session works.
        let mut extensions = get_enabled_extensions();
        extensions.extend(crate::plugins::mcp_servers::enabled_plugin_mcp_servers(
            Some(&session.working_dir),
        ));
        let extensions_state = EnabledExtensionsState::new(extensions);
        let mut extension_data = session.extension_data.clone();
        if let Err(e) = extensions_state.to_extension_data(&mut extension_data) {
            tracing::warn!(error = %e, "failed to initialize gateway session extensions");
        } else {
            update = update.extension_data(extension_data);
        }

        update.apply().await?;

        let now = chrono::Utc::now().timestamp();
        self.pairing_store
            .set(
                user,
                PairingState::Paired {
                    session_id: session.id.clone(),
                    paired_at: now,
                },
            )
            .await?;

        self.gateway
            .send_message(
                user,
                OutgoingMessage::Text {
                    body: "Paired! You can now chat with goose.".into(),
                },
            )
            .await?;

        Ok(())
    }

    /// Sync the session's provider, model, and extensions with the current
    /// global config so gateway sessions always reflect what the user has
    /// configured in the desktop app.  Returns `true` if extensions changed
    /// (which means the caller must recreate the agent so stale extension
    /// processes are torn down).
    async fn sync_session_config(&self, session: &Session) -> anyhow::Result<bool> {
        let config = Config::global();
        let manager = self.agent_manager.session_manager();

        // --- current global config ---
        let current_provider = config.get_goose_provider().ok();
        let current_model_name = config.get_goose_model().ok();
        let mut current_extensions = get_enabled_extensions();
        current_extensions.extend(crate::plugins::mcp_servers::enabled_plugin_mcp_servers(
            Some(&session.working_dir),
        ));
        let current_mode = config.get_goose_mode().unwrap_or_default();

        // --- what the session has ---
        let session_extensions: Vec<ExtensionConfig> =
            EnabledExtensionsState::from_extension_data(&session.extension_data)
                .map(|s| s.extensions)
                .unwrap_or_default();

        let provider_changed = current_provider.as_deref() != session.provider_name.as_deref();
        let model_changed = current_model_name.as_deref()
            != session.model_config.as_ref().map(|m| m.model_name.as_str());
        let extensions_changed = current_extensions != session_extensions;
        let mode_changed = current_mode != session.goose_mode;

        if !provider_changed && !model_changed && !extensions_changed && !mode_changed {
            return Ok(false);
        }

        tracing::info!(
            session_id = %session.id,
            provider_changed,
            model_changed,
            extensions_changed,
            mode_changed,
            "syncing gateway session with current config"
        );

        let mut update = manager.update(&session.id);

        if let Some(ref provider) = current_provider {
            update = update.provider_name(provider);
        }
        if let (Some(ref provider), Some(ref model_name)) = (&current_provider, &current_model_name)
        {
            if let Ok(model_config) =
                crate::model_config::model_config_from_user_config(provider, model_name)
            {
                update = update.model_config(model_config);
            }
        }

        if extensions_changed {
            let extensions_state = EnabledExtensionsState::new(current_extensions);
            let mut extension_data = session.extension_data.clone();
            if let Err(e) = extensions_state.to_extension_data(&mut extension_data) {
                tracing::warn!(error = %e, "failed to update gateway session extensions");
            } else {
                update = update.extension_data(extension_data);
            }
        }

        if mode_changed {
            update = update.goose_mode(current_mode);
        }

        update.apply().await?;
        Ok(extensions_changed)
    }

    async fn relay_to_session(
        &self,
        message: &IncomingMessage,
        session_id: &str,
    ) -> anyhow::Result<()> {
        self.gateway
            .send_message(&message.user, OutgoingMessage::Typing)
            .await?;

        let session = self
            .agent_manager
            .session_manager()
            .get_session(session_id, false)
            .await?;

        // Sync provider/model/extensions with the user's current desktop config.
        // If extensions changed we must tear down the old agent so stale
        // extension processes don't linger.
        let extensions_changed = self.sync_session_config(&session).await?;
        if extensions_changed {
            self.agent_manager
                .remove_session_if_loaded(session_id)
                .await?;
        }

        let agent = match self
            .agent_manager
            .get_or_create_agent(session_id.to_string())
            .await
        {
            Ok(agent) => agent,
            Err(error) if crate::acp::is_auth_required(&error) => {
                self.gateway
                    .send_message(
                        &message.user,
                        OutgoingMessage::Text {
                            body: format!("⚠️ Failed to configure provider: {error}"),
                        },
                    )
                    .await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        // Re-read the session after sync so restore picks up the new values.
        let session = self
            .agent_manager
            .session_manager()
            .get_session(session_id, false)
            .await?;

        // Ensure provider is configured (handles first use and LRU eviction).
        if let Err(e) = agent.restore_provider_from_session(&session).await {
            self.gateway
                .send_message(
                    &message.user,
                    OutgoingMessage::Text {
                        body: format!("⚠️ Failed to configure provider: {e}"),
                    },
                )
                .await?;
            return Ok(());
        }

        // Load extensions (skips any already loaded on the agent).
        agent.load_extensions_from_session(&session).await;

        let cancel = CancellationToken::new();
        let user_message = Message::user().with_text(&message.text);

        // Cap tool-calling loops so the agent doesn't run away doing
        // dozens of tool calls before responding.  After this many
        // LLM→tool round-trips the agent will stop and reply with
        // whatever it has.
        //
        // Honors `GOOSE_GATEWAY_MAX_TURNS` (gateway-specific override) and
        // `GOOSE_MAX_TURNS` (global), falling back to a conservative default
        // so the limit is configurable without editing the source.
        let config = Config::global();
        let max_turns = resolve_gateway_max_turns(
            config.get_param::<u32>("GOOSE_GATEWAY_MAX_TURNS").ok(),
            config.get_param::<u32>("GOOSE_MAX_TURNS").ok(),
        );

        let session_config = SessionConfig {
            id: session_id.to_string(),
            schedule_id: None,
            max_turns: Some(max_turns),
            retry_config: None,
        };

        let mut stream = match agent
            .reply(user_message, session_config, Some(cancel.clone()))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                self.gateway
                    .send_message(
                        &message.user,
                        OutgoingMessage::Text {
                            body: format!("⚠️ Failed to start agent: {e}"),
                        },
                    )
                    .await?;
                return Ok(());
            }
        };
        // Telegram stops showing "typing…" after ~5 seconds.  Re-send the
        // indicator every 4 s so the user always sees activity while the
        // agent is working (tool calls, LLM round-trips, etc.).
        let typing_cancel = CancellationToken::new();
        let typing_gateway = self.gateway.clone();
        let typing_user = message.user.clone();
        let typing_handle = tokio::spawn({
            let cancel = typing_cancel.clone();
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(4));
                interval.tick().await; // first tick is immediate, skip it
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = interval.tick() => {
                            if let Err(e) = typing_gateway
                                .send_message(&typing_user, OutgoingMessage::Typing)
                                .await
                            {
                                tracing::debug!(error = %e, "failed to re-send typing indicator");
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Buffer text within a single assistant message so we send one
        // Telegram message per LLM turn rather than per-chunk.  When a
        // ToolRequest appears in the same message we flush the buffer
        // first — the user sees "Let me check…" immediately, then the
        // typing indicator while the tool runs, then the next response.
        let mut pending_text = String::new();
        let mut sent_any = false;
        let mut event_count: u64 = 0;

        while let Some(event) = stream.next().await {
            event_count += 1;
            match event {
                Ok(AgentEvent::Message(ref msg)) => {
                    tracing::debug!(
                        session_id,
                        role = ?msg.role,
                        content_items = msg.content.len(),
                        "gateway stream: message event #{event_count}"
                    );
                    if msg.role == rmcp::model::Role::Assistant {
                        for content in &msg.content {
                            match content {
                                MessageContent::Text(t) => {
                                    if !t.text.is_empty() {
                                        pending_text.push_str(&t.text);
                                    }
                                }
                                MessageContent::ToolRequest(req) => {
                                    // Flush any accumulated text before
                                    // the tool runs — the user sees the
                                    // assistant's intent immediately.
                                    if !pending_text.is_empty() {
                                        let _ = self
                                            .gateway
                                            .send_message(
                                                &message.user,
                                                OutgoingMessage::Text {
                                                    body: std::mem::take(&mut pending_text),
                                                },
                                            )
                                            .await;
                                        sent_any = true;
                                    }
                                    if let Ok(call) = &req.tool_call {
                                        tracing::debug!(
                                            session_id,
                                            tool = %call.name,
                                            "gateway stream: tool request"
                                        );
                                        let _ = self
                                            .gateway
                                            .send_message(&message.user, OutgoingMessage::Typing)
                                            .await;
                                    }
                                }
                                MessageContent::ToolResponse(resp) => {
                                    tracing::debug!(
                                        session_id,
                                        id = %resp.id,
                                        success = resp.tool_result.is_ok(),
                                        "gateway stream: tool response"
                                    );
                                }
                                MessageContent::ActionRequired(action_required) => {
                                    if let ActionRequiredData::ToolConfirmation {
                                        id,
                                        tool_name,
                                        arguments,
                                        prompt,
                                    } = &action_required.data
                                    {
                                        // Flush pending text so the user sees context first.
                                        if !pending_text.is_empty() {
                                            let _ = self
                                                .gateway
                                                .send_message(
                                                    &message.user,
                                                    OutgoingMessage::Text {
                                                        body: std::mem::take(&mut pending_text),
                                                    },
                                                )
                                                .await;
                                        }

                                        let args_display = serde_json::to_string_pretty(arguments)
                                            .unwrap_or_default();
                                        let mut approval_text = format!(
                                            "🔐 Approval required\n\nTool: {tool_name}\nArguments:\n{args_display}"
                                        );
                                        if let Some(p) = prompt {
                                            approval_text.push_str(&format!("\n\n{p}"));
                                        }
                                        approval_text.push_str(
                                            "\n\nReply with:\n\
                                             • approve — allow once\n\
                                             • approve always — always allow\n\
                                             • deny — deny once\n\
                                             • deny always — always deny",
                                        );

                                        self.pending_confirmations
                                            .lock()
                                            .await
                                            .entry(message.user.clone())
                                            .and_modify(|pending| {
                                                pending.request_ids.push_back(id.clone());
                                            })
                                            .or_insert_with(|| PendingConfirmation {
                                                agent: agent.clone(),
                                                session_id: session_id.to_string(),
                                                request_ids: VecDeque::from([id.clone()]),
                                                cancel_token: cancel.clone(),
                                            });

                                        let send_result = self
                                            .gateway
                                            .send_message(
                                                &message.user,
                                                OutgoingMessage::Text {
                                                    body: approval_text,
                                                },
                                            )
                                            .await;

                                        if let Err(e) = send_result {
                                            tracing::error!(
                                                session_id,
                                                error = %e,
                                                "failed to deliver tool approval prompt; denying tool call"
                                            );
                                            let mut pending =
                                                self.pending_confirmations.lock().await;
                                            let remove_pending = pending
                                                .get_mut(&message.user)
                                                .is_some_and(|pending| {
                                                    pending
                                                        .request_ids
                                                        .retain(|request_id| request_id != id);
                                                    pending.request_ids.is_empty()
                                                });
                                            if remove_pending {
                                                pending.remove(&message.user);
                                            }
                                            drop(pending);
                                            if let Err(error) = agent
                                                .submit_tool_confirmation(
                                                    session_id,
                                                    id,
                                                    Permission::DenyOnce,
                                                )
                                                .await
                                            {
                                                tracing::error!(
                                                    session_id,
                                                    request_id = %id,
                                                    %error,
                                                    "failed to deny undeliverable tool approval"
                                                );
                                                cancel.cancel();
                                                self.pending_confirmations
                                                    .lock()
                                                    .await
                                                    .remove(&message.user);
                                            }
                                            sent_any = true;
                                        } else {
                                            sent_any = true;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok(AgentEvent::Usage(_)) => {}
                Ok(AgentEvent::MessageUsage { .. }) => {}
                Ok(AgentEvent::McpNotification(_)) => {
                    tracing::debug!(
                        session_id,
                        "gateway stream: mcp notification #{event_count}"
                    );
                }
                Ok(AgentEvent::HistoryReplaced(_)) => {
                    tracing::debug!(
                        session_id,
                        "gateway stream: history replaced #{event_count}"
                    );
                }
                Err(e) => {
                    tracing::error!(session_id, error = %e, "gateway stream: error at event #{event_count}");
                    // Stop typing indicator before sending error.
                    typing_cancel.cancel();
                    let _ = typing_handle.await;
                    self.gateway
                        .send_message(
                            &message.user,
                            OutgoingMessage::Text {
                                body: format!("⚠️ Agent error: {e}"),
                            },
                        )
                        .await?;
                    return Ok(());
                }
            }
        }

        // Stream finished — stop the typing indicator.
        typing_cancel.cancel();
        let _ = typing_handle.await;

        tracing::debug!(
            session_id,
            event_count,
            pending_text_len = pending_text.len(),
            sent_any,
            "gateway stream: finished"
        );

        // Send any remaining buffered text (this is typically the final
        // assistant response after the last tool round-trip).
        if !pending_text.is_empty() {
            self.gateway
                .send_message(&message.user, OutgoingMessage::Text { body: pending_text })
                .await?;
        } else if !sent_any {
            // Nothing was ever sent — let the user know.
            self.gateway
                .send_message(
                    &message.user,
                    OutgoingMessage::Text {
                        body: "(No response)".to_string(),
                    },
                )
                .await?;
        }

        Ok(())
    }
}

fn gateway_working_dir(platform: &str, user_id: &str) -> PathBuf {
    Paths::config_dir()
        .join("gateway")
        .join(platform)
        .join(user_id)
}

/// Operator-configured list of platform user IDs that may pair with the gateway,
/// read from `platform_config.allowed_user_ids`. An absent or empty list keeps
/// the default behavior of letting anyone attempt pairing.
fn allowed_user_ids_from_config(config: &GatewayConfig) -> Option<HashSet<String>> {
    let ids: HashSet<String> = config.platform_config["allowed_user_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| {
            id.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| id.as_u64().map(|n| n.to_string()))
        })
        .collect();
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

fn is_user_allowed(allowed: &Option<HashSet<String>>, user: &PlatformUser) -> bool {
    match allowed {
        None => true,
        Some(ids) => ids.contains(&user.user_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_overrides() {
        assert_eq!(
            resolve_gateway_max_turns(None, None),
            DEFAULT_GATEWAY_MAX_TURNS
        );
    }

    #[test]
    fn uses_global_max_turns_when_gateway_unset() {
        assert_eq!(resolve_gateway_max_turns(None, Some(42)), 42);
    }

    #[test]
    fn gateway_override_wins_over_global() {
        assert_eq!(resolve_gateway_max_turns(Some(10), Some(42)), 10);
    }

    #[test]
    fn gateway_override_used_when_global_unset() {
        assert_eq!(resolve_gateway_max_turns(Some(25), None), 25);
    }

    fn platform_user(user_id: &str) -> PlatformUser {
        PlatformUser {
            platform: "telegram".to_string(),
            user_id: user_id.to_string(),
            display_name: None,
        }
    }

    fn config_with_platform(platform_config: serde_json::Value) -> GatewayConfig {
        GatewayConfig {
            gateway_type: "telegram".to_string(),
            platform_config,
            max_sessions: 1,
        }
    }

    #[test]
    fn allowlist_absent_or_empty_allows_everyone() {
        for platform_config in [
            serde_json::json!({}),
            serde_json::json!({"allowed_user_ids": []}),
        ] {
            let allowed = allowed_user_ids_from_config(&config_with_platform(platform_config));
            assert!(allowed.is_none());
            assert!(is_user_allowed(&allowed, &platform_user("999999")));
        }
    }

    #[test]
    fn allowlist_admits_listed_and_blocks_unlisted_users() {
        let config = config_with_platform(serde_json::json!({"allowed_user_ids": ["111", 222]}));
        let allowed = allowed_user_ids_from_config(&config).expect("allowlist should be set");
        assert_eq!(allowed.len(), 2);
        assert!(is_user_allowed(
            &Some(allowed.clone()),
            &platform_user("111")
        ));
        assert!(is_user_allowed(&Some(allowed), &platform_user("222")));

        let allowed = allowed_user_ids_from_config(&config).expect("allowlist should be set");
        assert!(!is_user_allowed(&Some(allowed), &platform_user("333")));
    }

    #[test]
    fn allowlist_ignores_non_id_values() {
        let config = config_with_platform(serde_json::json!({"allowed_user_ids": [true, null]}));
        assert!(allowed_user_ids_from_config(&config).is_none());
    }
}
