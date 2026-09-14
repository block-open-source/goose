use anyhow::Result;
use async_trait::async_trait;

use crate::agents::state_machine::effects::GooseEffect;
use crate::agents::state_machine::usage;
use crate::agents::AgentEvent;
use crate::conversation::message::{ActionRequiredData, Message, MessageContent};
use crate::conversation::Conversation;
use crate::session::{Session, SessionManager};
use goose_agent::machine::{EffectHandler, EffectUsage, MachineSession, SessionLoader};
use goose_agent::operation::{ConversationEffect, Emitter, MachineEffect};

fn contains_tool_confirmation_request(message: &Message) -> bool {
    message.content.iter().any(|content| {
        matches!(
            content,
            MessageContent::ActionRequired(action)
                if matches!(&action.data, ActionRequiredData::ToolConfirmation { .. })
        )
    })
}

#[derive(Clone)]
pub(crate) struct RunScope {
    pub kickoff_message_id: String,
}

fn scoped_session(mut session: Session, scope: Option<&RunScope>) -> Result<Session> {
    let Some(conversation) = session.conversation.as_mut() else {
        return Ok(session);
    };
    for message in conversation.messages_mut() {
        if is_live_transcript(message) {
            message.metadata.agent_visible = false;
        }
    }
    let Some(scope) = scope else {
        return Ok(session);
    };
    let kickoff = conversation
        .messages()
        .iter()
        .position(|message| message.id.as_deref() == Some(&scope.kickoff_message_id))
        .ok_or_else(|| anyhow::anyhow!("state machine kickoff message is missing"))?;
    let mut index = 0;
    conversation.messages_mut().retain_mut(|message| {
        let live_transcript = is_live_transcript(message);
        let keep = !live_transcript && (index <= kickoff || message.is_agent_visible());
        index += 1;
        keep
    });
    Ok(session)
}

fn is_live_transcript(message: &crate::conversation::message::Message) -> bool {
    message
        .metadata
        .operation_note("live_voice", "transcript")
        .is_some_and(|value| value.as_bool() == Some(true))
}

fn scope_replacements(effects: &mut [GooseEffect], scope: Option<&RunScope>) {
    if scope.is_none() {
        return;
    }
    for effect in effects {
        let replacement = match effect {
            GooseEffect::Conversation(ConversationEffect::ReplaceConversation(conversation)) => {
                Some((std::mem::take(conversation), None))
            }
            GooseEffect::ReplaceConversation {
                conversation,
                usage,
            } => Some((std::mem::take(conversation), usage.take())),
            _ => None,
        };
        let Some((conversation, usage)) = replacement else {
            continue;
        };
        *effect = GooseEffect::ReplaceScopedConversation {
            conversation,
            usage,
        };
    }
}

impl MachineSession for Session {
    fn id(&self) -> &str {
        &self.id
    }
    fn conversation(&self) -> Option<&Conversation> {
        self.conversation.as_ref()
    }
}

#[async_trait]
impl SessionLoader<Session> for SessionManager {
    async fn load(&self, session_id: &str) -> Result<Session> {
        self.get_session(session_id, true).await
    }
}

#[async_trait]
impl EffectHandler<Session, GooseEffect> for SessionManager {
    async fn apply_effects(
        &self,
        session: &Session,
        effects: &mut [GooseEffect],
        emit: &Emitter,
    ) -> Result<()> {
        for effect in effects.iter_mut() {
            effect.ensure_message_ids();
        }
        usage::enrich(session, effects);

        for effect in effects.iter_mut() {
            match effect {
                GooseEffect::Conversation(ConversationEffect::AppendMessage(message)) => {
                    self.add_message(&session.id, message).await?;
                }
                GooseEffect::Conversation(ConversationEffect::ReplaceConversation(
                    conversation,
                )) => {
                    self.replace_conversation(&session.id, conversation).await?;
                    self.update(&session.id)
                        .usage(usage::estimate_context(conversation).await?)
                        .apply()
                        .await?;
                }
                GooseEffect::ReplaceConversation {
                    conversation,
                    usage: replacement_usage,
                } => {
                    if let Some(provider_usage) = replacement_usage {
                        usage::record(self, session, provider_usage, true).await?;
                    }
                    self.replace_conversation(&session.id, conversation).await?;
                    self.update(&session.id)
                        .usage(usage::estimate_context(conversation).await?)
                        .apply()
                        .await?;
                }
                GooseEffect::ReplaceScopedConversation {
                    conversation,
                    usage: replacement_usage,
                } => {
                    if let Some(provider_usage) = replacement_usage {
                        usage::record(self, session, provider_usage, true).await?;
                    }
                    let source_message_ids = session
                        .conversation()
                        .into_iter()
                        .flat_map(Conversation::messages)
                        .filter_map(|message| message.id.clone())
                        .collect();
                    self.replace_scoped_conversation(
                        &session.id,
                        conversation,
                        &source_message_ids,
                    )
                    .await?;
                    self.update(&session.id)
                        .usage(usage::estimate_context(conversation).await?)
                        .apply()
                        .await?;
                }
                GooseEffect::Conversation(ConversationEffect::PatchToolRequestMeta {
                    tool_call_id,
                    patch,
                }) => {
                    self.update_tool_request_meta(&session.id, tool_call_id, patch.clone())
                        .await?;
                }
                GooseEffect::Conversation(ConversationEffect::SetMessageVisibility {
                    message_id,
                    user_visible,
                    agent_visible,
                }) => {
                    self.update_message_metadata(&session.id, message_id, |mut metadata| {
                        metadata.user_visible = *user_visible;
                        metadata.agent_visible = *agent_visible;
                        metadata
                    })
                    .await?;
                }
                GooseEffect::SetRecipe(recipe) => {
                    self.update(&session.id)
                        .recipe(recipe.as_ref().clone())
                        .apply()
                        .await?;
                }
                GooseEffect::SetExtensionData(extension_data) => {
                    self.update(&session.id)
                        .extension_data(extension_data.clone())
                        .apply()
                        .await?;
                }
                GooseEffect::RecordUsage(provider_usage) => {
                    usage::record(self, session, provider_usage, false).await?;
                }
            }
        }

        for effect in effects {
            match effect {
                GooseEffect::Conversation(ConversationEffect::AppendMessage(message)) => {
                    if contains_tool_confirmation_request(message) {
                        // Responses can arrive immediately, so publish only after the persistence pass.
                        emit.emit(AgentEvent::Message(message.clone())).await;
                    }
                    if let Some(usage) = message
                        .metadata
                        .usage
                        .as_deref()
                        .filter(|_| !message.user_visible_content().content.is_empty())
                        .cloned()
                    {
                        emit.emit(AgentEvent::MessageUsage {
                            message_id: message.id.clone(),
                            usage,
                        })
                        .await;
                    }
                }
                GooseEffect::Conversation(ConversationEffect::ReplaceConversation(
                    conversation,
                ))
                | GooseEffect::ReplaceConversation { conversation, .. }
                | GooseEffect::ReplaceScopedConversation { conversation, .. } => {
                    emit.emit(AgentEvent::HistoryReplaced(conversation.clone()))
                        .await;
                }
                GooseEffect::RecordUsage(usage) => {
                    emit.emit(AgentEvent::Usage(usage.clone())).await
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl EffectUsage<GooseEffect> for SessionManager {
    fn usage(
        &self,
        effect: &GooseEffect,
    ) -> Option<goose_providers::conversation::token_usage::Usage> {
        match effect {
            GooseEffect::RecordUsage(usage)
            | GooseEffect::ReplaceConversation {
                usage: Some(usage), ..
            }
            | GooseEffect::ReplaceScopedConversation {
                usage: Some(usage), ..
            } => Some(usage.usage),
            _ => None,
        }
    }
}

pub(crate) async fn run(
    machine: &crate::agents::state_machine::StateMachine<'_, Session, GooseEffect>,
    runtime: &SessionManager,
    session_id: &str,
    emit: &Emitter,
    scope: Option<&RunScope>,
) -> Result<Session> {
    let entry_session = scoped_session(runtime.load(session_id).await?, scope)?;
    tracing::Span::current().record(
        "gen_ai.agent.name",
        crate::agents::gen_ai_telemetry::agent_name(&entry_session),
    );
    let trace_input = if crate::agents::gen_ai_telemetry::capture_message_content() {
        entry_session
            .conversation()
            .and_then(|conversation| {
                crate::agents::state_machine::messages_since_kickoff(conversation).ok()
            })
            .and_then(|messages| messages.first())
            .map(crate::conversation::message::Message::user_visible_content)
            .map(|message| message.as_concat_text())
            .filter(|text| !text.is_empty())
    } else {
        None
    };
    if let Some(input) = trace_input {
        tracing::Span::current().record("trace_input", input.as_str());
    }

    let mut turn_usage = goose_providers::conversation::token_usage::Usage::default();
    loop {
        let session = scoped_session(runtime.load(session_id).await?, scope)?;
        let Some(mut result) = machine.step(&session, emit).await? else {
            break;
        };
        tracing::debug!(target: "goose::state_machine", step = result.applied_step, "applied step");
        scope_replacements(&mut result.effects, scope);
        for effect in &result.effects {
            if let Some(usage) = runtime.usage(effect) {
                turn_usage += usage;
            }
        }
        machine.apply(runtime, &session, &mut result, emit).await?;
        if result.yield_to_client {
            break;
        }
    }

    let session = scoped_session(runtime.load(session_id).await?, scope)?;
    let last_assistant_text = session
        .conversation()
        .and_then(|conversation| {
            crate::agents::state_machine::messages_since_kickoff(conversation).ok()
        })
        .into_iter()
        .flatten()
        .rev()
        .filter(|message| message.role == rmcp::model::Role::Assistant)
        .map(crate::conversation::message::Message::user_visible_content)
        .map(|message| message.as_concat_text())
        .find(|text| !text.is_empty())
        .unwrap_or_default();
    if !last_assistant_text.is_empty() {
        let span = tracing::Span::current();
        if crate::agents::gen_ai_telemetry::capture_message_content() {
            span.record("trace_output", last_assistant_text.as_str());
            let output = crate::agents::gen_ai_telemetry::simple_output_json(&last_assistant_text);
            span.record("gen_ai.output.messages", output.as_str());
        }
    }
    crate::agents::gen_ai_telemetry::record_usage(&tracing::Span::current(), &turn_usage);
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GooseMode;
    use crate::conversation::message::{Message, MessageMetadata};
    use crate::session::session_manager::SessionType;

    #[tokio::test]
    async fn normal_session_keeps_live_transcript_out_of_provider_history() {
        let manager = SessionManager::new(tempfile::tempdir().unwrap().keep());
        let session = manager
            .create_session(
                "/tmp".into(),
                "live transcript visibility".into(),
                SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        let mut transcript_metadata = MessageMetadata::default();
        transcript_metadata.set_operation_note(
            "live_voice",
            "transcript",
            serde_json::Value::Bool(true),
        );
        manager
            .add_message(
                &session.id,
                &Message::assistant()
                    .with_id("live-transcript")
                    .with_text("interleaved with a tool call")
                    .with_metadata(transcript_metadata),
            )
            .await
            .unwrap();

        let loaded =
            scoped_session(manager.get_session(&session.id, true).await.unwrap(), None).unwrap();
        let transcript = loaded
            .conversation
            .unwrap()
            .messages()
            .iter()
            .find(|message| message.id.as_deref() == Some("live-transcript"))
            .cloned()
            .unwrap();

        assert!(transcript.is_user_visible());
        assert!(!transcript.is_agent_visible());
    }

    #[tokio::test]
    async fn live_scope_keeps_agent_work_and_excludes_live_transcript() {
        let manager = SessionManager::new(tempfile::tempdir().unwrap().keep());
        let session = manager
            .create_session(
                "/tmp".into(),
                "one session".into(),
                SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        let mut transcript_metadata = MessageMetadata::default();
        transcript_metadata.set_operation_note(
            "live_voice",
            "transcript",
            serde_json::Value::Bool(true),
        );
        manager
            .add_message(
                &session.id,
                &Message::user()
                    .with_id("earlier-live")
                    .with_text("delegated speech already copied into the kickoff")
                    .with_metadata(transcript_metadata.clone()),
            )
            .await
            .unwrap();
        let kickoff = Message::user()
            .with_id("kickoff")
            .with_text("delegated request")
            .agent_only();
        manager.add_message(&session.id, &kickoff).await.unwrap();
        manager
            .add_message(
                &session.id,
                &Message::user()
                    .with_id("later-live")
                    .with_text("unrelated speech")
                    .with_metadata(transcript_metadata),
            )
            .await
            .unwrap();
        manager
            .add_message(
                &session.id,
                &Message::assistant()
                    .with_id("agent-work")
                    .with_text("coding result"),
            )
            .await
            .unwrap();

        let scoped = scoped_session(
            manager.get_session(&session.id, true).await.unwrap(),
            Some(&RunScope {
                kickoff_message_id: "kickoff".into(),
            }),
        )
        .unwrap();
        let messages = scoped.conversation.unwrap();
        assert!(messages
            .messages()
            .iter()
            .any(|message| message.id.as_deref() == Some("kickoff")));
        assert!(messages
            .messages()
            .iter()
            .any(|message| message.id.as_deref() == Some("agent-work")));
        let kickoff = messages
            .messages()
            .iter()
            .find(|message| message.id.as_deref() == Some("kickoff"))
            .unwrap();
        assert!(!kickoff.is_user_visible());
        assert!(kickoff.is_agent_visible());
        assert!(!messages
            .messages()
            .iter()
            .any(|message| message.id.as_deref() == Some("earlier-live")));
        assert!(!messages
            .messages()
            .iter()
            .any(|message| message.id.as_deref() == Some("later-live")));
    }

    #[tokio::test]
    async fn scoped_replacement_preserves_transcript_and_hidden_handoff() {
        let manager = SessionManager::new(tempfile::tempdir().unwrap().keep());
        let session = manager
            .create_session(
                "/tmp".into(),
                "scoped compaction".into(),
                SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        let kickoff = Message::user()
            .with_id("kickoff")
            .with_text("delegated request")
            .agent_only();
        manager.add_message(&session.id, &kickoff).await.unwrap();

        let full = manager.get_session(&session.id, true).await.unwrap();
        let scope = RunScope {
            kickoff_message_id: "kickoff".into(),
        };
        let scoped = scoped_session(full, Some(&scope)).unwrap();

        let mut transcript_metadata = MessageMetadata::default();
        transcript_metadata.set_operation_note(
            "live_voice",
            "transcript",
            serde_json::Value::Bool(true),
        );
        manager
            .add_message(
                &session.id,
                &Message::user()
                    .with_id("concurrent-live")
                    .with_text("still speaking")
                    .with_metadata(transcript_metadata),
            )
            .await
            .unwrap();

        let mut compacted = scoped.conversation.clone().unwrap();
        for message in compacted.messages_mut() {
            message.metadata.agent_visible = false;
        }
        compacted.messages_mut().push(
            Message::assistant()
                .with_id("summary")
                .with_text("summary")
                .agent_only(),
        );
        let mut effects = [GooseEffect::ReplaceConversation {
            conversation: compacted,
            usage: None,
        }];
        scope_replacements(&mut effects, Some(&scope));
        let GooseEffect::ReplaceScopedConversation { conversation, .. } = &effects[0] else {
            panic!("replacement should be scoped");
        };
        let source_message_ids = scoped
            .conversation()
            .unwrap()
            .messages()
            .iter()
            .filter_map(|message| message.id.clone())
            .collect();
        manager
            .replace_scoped_conversation(&session.id, conversation, &source_message_ids)
            .await
            .unwrap();

        let reloaded = manager.get_session(&session.id, true).await.unwrap();
        let messages = reloaded.conversation.unwrap();
        let kickoff = messages
            .messages()
            .iter()
            .find(|message| message.id.as_deref() == Some("kickoff"))
            .unwrap();
        assert!(!kickoff.is_user_visible());
        assert!(!kickoff.is_agent_visible());
        assert!(messages
            .messages()
            .iter()
            .any(|message| message.id.as_deref() == Some("concurrent-live")));
        assert!(messages
            .messages()
            .iter()
            .any(|message| message.id.as_deref() == Some("summary")));
    }
}
