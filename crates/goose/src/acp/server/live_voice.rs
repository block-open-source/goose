mod call;
mod service;

use super::*;
use call::{LiveVoiceCallId, LiveVoiceCallState};
use futures::FutureExt;
use service::{
    wait_until_finished, LiveVoiceAvailability, LiveVoiceCallEndedHandler,
    LiveVoiceDelegationHandler, LiveVoiceError, LiveVoiceTranscriptHandler, WebRtcOffer,
};

pub use service::LiveVoiceService;

const LIVE_DELEGATION_SUMMARY_INSTRUCTION: &str = "End every run with a concise, user-facing summary below 400 tokens. State the outcome, what changed, validation performed, failures, and any required user action. Do not include raw tool output.";

impl GooseAcpAgent {
    async fn load_live_voice_session(
        &self,
        session_id: &str,
    ) -> Result<Session, agent_client_protocol::Error> {
        self.session_manager
            .get_session(session_id, false)
            .await
            .map_err(|_| {
                agent_client_protocol::Error::resource_not_found(None)
                    .data("Session is not accessible")
            })
    }

    pub(super) async fn on_live_voice_availability(
        &self,
        req: LiveVoiceAvailabilityRequest,
    ) -> Result<LiveVoiceAvailabilityResponse, agent_client_protocol::Error> {
        let session = self.load_live_voice_session(&req.session_id).await?;
        let status = match self
            .live_voice
            .availability(&req.session_id, session.goose_mode)
        {
            LiveVoiceAvailability::Ready => LiveVoiceStatus::Ready,
            LiveVoiceAvailability::FeatureDisabled => LiveVoiceStatus::FeatureDisabled,
            LiveVoiceAvailability::ProviderUnavailable => LiveVoiceStatus::ProviderUnavailable,
            LiveVoiceAvailability::ChatBusy => LiveVoiceStatus::SessionBusy,
            LiveVoiceAvailability::RequiresAutonomousMode => {
                LiveVoiceStatus::RequiresAutonomousMode
            }
        };
        Ok(LiveVoiceAvailabilityResponse { status })
    }

    pub(super) async fn on_live_voice_start(
        self: &Arc<Self>,
        cx: &ConnectionTo<Client>,
        req: LiveVoiceStartRequest,
    ) -> Result<LiveVoiceStartResponse, agent_client_protocol::Error> {
        let offer = WebRtcOffer::new(req.offer_sdp)
            .ok_or_else(agent_client_protocol::Error::invalid_params)?;
        let call_ended_handler: LiveVoiceCallEndedHandler =
            if self.supports_goose_custom_notifications() {
                let notification_connection = cx.clone();
                Arc::new(move |ended| {
                    let outcome = match ended.state {
                        LiveVoiceCallState::Stopped => LiveVoiceCallOutcome::Stopped,
                        LiveVoiceCallState::Failed => LiveVoiceCallOutcome::Failed,
                        // An invalid ended event must still tell Desktop to release local media.
                        LiveVoiceCallState::Live => LiveVoiceCallOutcome::Failed,
                    };
                    let _ = notification_connection.send_notification(GooseSessionNotification {
                        session_id: ended.session_id,
                        update: GooseSessionUpdate::LiveVoiceCallEnded(LiveVoiceCallEndedUpdate {
                            call_id: ended.call_id.0,
                            outcome,
                        }),
                    });
                })
            } else {
                Arc::new(|_| {})
            };

        let session_id = req.session_id.clone();
        let transcript_session_id = req.session_id.clone();
        let transcript_connection = cx.clone();
        let transcript_handler: LiveVoiceTranscriptHandler = Arc::new(move |message| {
            let [MessageContent::Text(text)] = message.content.as_slice() else {
                return;
            };
            let chunk = content_chunk_for_message(
                &message,
                ContentBlock::Text(TextContent::new(text.text.clone())),
            );
            let update = match message.role {
                Role::User => SessionUpdate::UserMessageChunk(chunk),
                Role::Assistant => SessionUpdate::AgentMessageChunk(chunk),
            };
            let _ = transcript_connection.send_notification(SessionNotification::new(
                SessionId::new(transcript_session_id.clone()),
                update,
            ));
        });
        let delegation_agent = Arc::clone(self);
        let delegation_handler: LiveVoiceDelegationHandler =
            Arc::new(move |main_session_id, input, cancellation| {
                let agent = delegation_agent.clone();
                async move {
                    agent
                        .run_live_delegation(main_session_id, input, cancellation)
                        .await
                }
                .boxed()
            });
        let start = self.live_voice.start_call(
            &req.session_id,
            offer,
            self.session_manager.clone(),
            transcript_handler,
            call_ended_handler,
            delegation_handler,
        );
        tokio::pin!(start);
        let call = tokio::select! {
            result = &mut start => result.map_err(map_live_voice_error)?,
            _ = cx.incoming_closed() => {
                return Err(agent_client_protocol::Error::internal_error()
                    .data("ACP connection closed while Live voice was starting"));
            }
        };

        let call_id = call.call_id.clone();
        let live_voice = self.live_voice.clone();
        let finished_state_rx = call.finished_state_rx;
        let watcher_connection = cx.clone();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = watcher_connection.incoming_closed() => {
                    let _ = live_voice.stop_call(&session_id, &call_id).await;
                }
                _ = wait_until_finished(finished_state_rx) => {}
            }
        });

        Ok(LiveVoiceStartResponse {
            call_id: call.call_id.0,
            answer_sdp: call.answer.into_sdp(),
        })
    }

    pub(super) async fn on_live_voice_stop(
        &self,
        req: LiveVoiceStopRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        self.load_live_voice_session(&req.session_id).await?;
        let call_id = LiveVoiceCallId(req.call_id);
        self.live_voice
            .stop_call(&req.session_id, &call_id)
            .await
            .map_err(map_live_voice_error)?;
        Ok(EmptyResponse {})
    }

    async fn run_live_delegation(
        &self,
        main_session_id: String,
        input: String,
        cancel_token: CancellationToken,
    ) -> String {
        let (linked_session, agent) =
            match self.prepare_live_delegated_session(&main_session_id).await {
                Ok(value) => value,
                Err(message) => return message,
            };
        let linked_session_id = linked_session.id.clone();
        let run_id = format!("run_{}", Uuid::new_v4());
        if self
            .start_active_run(
                &linked_session_id,
                run_id.clone(),
                cancel_token.clone(),
                agent.clone(),
            )
            .await
            .is_err()
        {
            return "The coding task is busy right now.".into();
        }
        let _run_guard = ActiveRunDropGuard {
            registry: self.active_runs.clone(),
            session_id: linked_session_id.clone(),
            run_id: run_id.clone(),
            cancel_token: cancel_token.clone(),
        };

        let session_config = SessionConfig {
            id: linked_session_id.clone(),
            schedule_id: None,
            max_turns: None,
            retry_config: None,
        };
        let mut stream = match agent
            .reply(
                Message::user().with_text(input),
                session_config,
                Some(cancel_token.clone()),
            )
            .await
        {
            Ok(stream) => stream,
            Err(_) => {
                self.clear_active_run(&linked_session_id, &run_id).await;
                return "The coding task failed before it could start.".into();
            }
        };

        let mut outcome_message_id: Option<String> = None;
        let mut outcome = String::new();
        while let Some(event) = stream.next().await {
            if cancel_token.is_cancelled() {
                self.clear_active_run(&linked_session_id, &run_id).await;
                return "The coding task was cancelled.".into();
            }
            match event {
                Ok(crate::agents::AgentEvent::Message(message))
                    if message.role == Role::Assistant && message.is_user_visible() =>
                {
                    let text = message
                        .user_visible_content()
                        .content
                        .iter()
                        .filter_map(MessageContent::as_text)
                        .collect::<String>();
                    if !text.trim().is_empty() {
                        if outcome_message_id.is_some() && outcome_message_id == message.id {
                            outcome.push_str(&text);
                        } else {
                            outcome_message_id = message.id;
                            outcome = text;
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    self.clear_active_run(&linked_session_id, &run_id).await;
                    return "The coding task failed before it could complete.".into();
                }
            }
        }

        self.clear_active_run(&linked_session_id, &run_id).await;
        if cancel_token.is_cancelled() {
            return "The coding task was cancelled.".into();
        }
        if outcome.is_empty() {
            return "The coding task finished without a user-facing result.".into();
        }
        if self
            .persist_live_delegation_outcome(&main_session_id, &outcome)
            .await
            .is_err()
        {
            return "The coding task completed, but Goose could not save its result.".into();
        }
        outcome
    }

    async fn prepare_live_delegated_session(
        &self,
        main_session_id: &str,
    ) -> Result<(Session, Arc<Agent>), String> {
        let main_session = self
            .session_manager
            .get_session(main_session_id, false)
            .await
            .map_err(|_| "The coding task could not load this chat.".to_string())?;
        let provider_name = main_session
            .provider_name
            .clone()
            .ok_or_else(|| "The coding task has no selected provider.".to_string())?;
        let model_config = main_session
            .model_config
            .clone()
            .ok_or_else(|| "The coding task has no selected model.".to_string())?;
        let existing = self
            .session_manager
            .find_child_session(main_session_id, SessionType::User)
            .await
            .map_err(|_| "Goose found an invalid delegated-session setup.".to_string())?;
        let mut linked_session = match existing {
            Some(session) => {
                if self.active_runs.is_active(&session.id) {
                    return Err("The coding task is busy right now.".into());
                }
                let session_with_messages = self
                    .session_manager
                    .get_session(&session.id, true)
                    .await
                    .map_err(|_| "The coding task could not load its work session.".to_string())?;
                if session_with_messages
                    .conversation
                    .as_ref()
                    .is_some_and(|conversation| !conversation.messages().is_empty())
                {
                    return Err("Continuation is not yet available.".into());
                }
                session
            }
            None => {
                let session = self
                    .session_manager
                    .create_session(
                        main_session.working_dir.clone(),
                        format!("Live work for {}", main_session.name),
                        SessionType::User,
                        GooseMode::Auto,
                    )
                    .await
                    .map_err(|_| "Goose could not create the coding work session.".to_string())?;
                self.session_manager
                    .update(&session.id)
                    .parent_session_id(Some(main_session_id.to_string()))
                    .apply()
                    .await
                    .map_err(|_| "Goose could not link the coding work session.".to_string())?;
                session
            }
        };
        let configuration_changed = linked_session.working_dir != main_session.working_dir
            || linked_session.provider_name.as_deref() != Some(provider_name.as_str())
            || linked_session
                .model_config
                .as_ref()
                .and_then(|config| serde_json::to_value(config).ok())
                != serde_json::to_value(&model_config).ok()
            || linked_session.goose_mode != GooseMode::Auto
            || linked_session.project_id != main_session.project_id
            || linked_session.extension_data.extension_states
                != main_session.extension_data.extension_states;
        if configuration_changed {
            self.session_manager
                .update(&linked_session.id)
                .working_dir(main_session.working_dir)
                .provider_name(provider_name)
                .model_config(model_config)
                .goose_mode(GooseMode::Auto)
                .project_id(main_session.project_id)
                .extension_data(main_session.extension_data)
                .apply()
                .await
                .map_err(|_| "Goose could not configure the coding work session.".to_string())?;
            self.sessions.lock().await.remove(&linked_session.id);
            self.agent_manager
                .remove_session_if_loaded(&linked_session.id)
                .await
                .map_err(|_| "Goose could not refresh the coding agent.".to_string())?;
            linked_session = self
                .session_manager
                .get_session(&linked_session.id, false)
                .await
                .map_err(|_| "Goose could not reload the coding work session.".to_string())?;
        }

        let agent = self
            .get_session_agent(&linked_session.id)
            .await
            .map_err(|_| "Goose could not activate the coding agent.".to_string())?;
        agent
            .extend_system_prompt(
                "live_delegation_summary".into(),
                LIVE_DELEGATION_SUMMARY_INSTRUCTION.into(),
            )
            .await;
        Ok((linked_session, agent))
    }

    async fn persist_live_delegation_outcome(
        &self,
        main_session_id: &str,
        outcome: &str,
    ) -> anyhow::Result<()> {
        let mut metadata = crate::conversation::message::MessageMetadata::agent_only();
        metadata.set_operation_note("live_delegation", "outcome", serde_json::Value::Bool(true));
        self.session_manager
            .add_message(
                main_session_id,
                &Message::assistant()
                    .with_text(outcome)
                    .with_metadata(metadata),
            )
            .await
    }
}

fn map_live_voice_error(error: LiveVoiceError) -> agent_client_protocol::Error {
    match error {
        LiveVoiceError::Unavailable => {
            agent_client_protocol::Error::invalid_params().data("Live voice is unavailable")
        }
        LiveVoiceError::StartFailed => {
            agent_client_protocol::Error::internal_error().data("Live voice could not start")
        }
        LiveVoiceError::StopFailed => {
            agent_client_protocol::Error::internal_error().data("Live voice could not stop cleanly")
        }
    }
}
