mod call;
mod service;

use super::*;
use call::LiveVoiceCallId;
use futures::FutureExt;
use service::{
    wait_for_completion, LiveVoiceAvailability, LiveVoiceCallCompletion, LiveVoiceCallEndedHandler,
    LiveVoiceDelegationCommand, LiveVoiceDelegationHandler, LiveVoiceError,
    LiveVoiceTranscriptHandler, WebRtcOffer,
};

pub use service::LiveVoiceService;

const LIVE_DELEGATION_INSTRUCTION: &str = concat!(
    "You support a live conversation. Treat the latest delegated input as an update to this session's earlier context. ",
    "Voice transcripts may be incomplete. If required information is missing, ask a brief clarification instead of guessing. ",
    "End every run with a concise, user-facing result below 400 tokens. State the outcome, what changed, validation performed, ",
    "failures, and any required user action. Do not include raw tool output."
);

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
                    let outcome = match ended.completion {
                        LiveVoiceCallCompletion::Stopped => LiveVoiceCallOutcome::Stopped,
                        LiveVoiceCallCompletion::Failed => LiveVoiceCallOutcome::Failed,
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
        let prepared_agent = self.prepare_live_agent(&req.session_id).await;
        let delegation_owner = Arc::clone(self);
        let delegation_connection = cx.clone();
        let delegation_handler: LiveVoiceDelegationHandler =
            Arc::new(move |session_id, delegation| {
                let owner = delegation_owner.clone();
                let connection = delegation_connection.clone();
                match delegation {
                    LiveVoiceDelegationCommand::Steer { input } => {
                        async move { owner.steer_live_delegation(&session_id, input).await }.boxed()
                    }
                    LiveVoiceDelegationCommand::Start { input } => owner.start_live_delegation(
                        session_id,
                        input,
                        connection,
                        prepared_agent.clone(),
                    ),
                }
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
        let completion_rx = call.completion_rx;
        let watcher_connection = cx.clone();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = watcher_connection.incoming_closed() => {
                    let _ = live_voice.stop_call(&session_id, &call_id).await;
                }
                _ = wait_for_completion(completion_rx) => {}
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

    fn start_live_delegation(
        self: Arc<Self>,
        session_id: String,
        input: String,
        cx: ConnectionTo<Client>,
        prepared_agent: Result<Arc<Agent>, String>,
    ) -> BoxFuture<'static, String> {
        let agent = match prepared_agent {
            Ok(value) => value,
            Err(message) => return async move { message }.boxed(),
        };
        let cancel_token = CancellationToken::new();
        let run_id = format!("run_{}", Uuid::new_v4());
        if self
            .active_runs
            .start_live_delegation(
                &session_id,
                run_id.clone(),
                cancel_token.clone(),
                agent.clone(),
            )
            .is_err()
        {
            return async { "The coding task is busy right now.".into() }.boxed();
        }
        let run_guard = ActiveRunDropGuard {
            registry: self.active_runs.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            cancel_token: cancel_token.clone(),
        };

        async move {
            let _run_guard = run_guard;
            self.run_live_delegation(session_id, input, cancel_token, cx, agent, run_id)
                .await
        }
        .boxed()
    }

    async fn run_live_delegation(
        &self,
        session_id: String,
        input: String,
        cancel_token: CancellationToken,
        cx: ConnectionTo<Client>,
        agent: Arc<Agent>,
        run_id: String,
    ) -> String {
        let acp_session_id = SessionId::new(session_id.clone());
        let _ = Self::send_active_run_update(&cx, &acp_session_id, Some(&run_id));

        let session_config = SessionConfig {
            id: session_id.clone(),
            schedule_id: None,
            max_turns: None,
            retry_config: None,
        };
        let input_message =
            Message::user().with_text(format!("{LIVE_DELEGATION_INSTRUCTION}\n\n{input}"));
        let mut stream = match agent
            .reply_live_delegation(input_message, session_config, cancel_token.clone())
            .await
        {
            Ok(stream) => stream,
            Err(_) => {
                self.clear_active_run(&session_id, &run_id).await;
                let _ = Self::send_active_run_update(&cx, &acp_session_id, None);
                return "The coding task failed before it could start.".into();
            }
        };

        let mut tool_requests = HashMap::new();
        let mut outcome_message_id: Option<String> = None;
        let mut outcome = String::new();
        while let Some(event) = stream.next().await {
            if cancel_token.is_cancelled() {
                self.clear_active_run(&session_id, &run_id).await;
                let _ = Self::send_active_run_update(&cx, &acp_session_id, None);
                return "The coding task was cancelled.".into();
            }
            match event {
                Ok(crate::agents::AgentEvent::Message(message)) => {
                    let is_user_visible = message.is_user_visible();
                    let is_visible_assistant = message.role == Role::Assistant && is_user_visible;
                    let projected_message = message.user_visible_content();
                    for content in &projected_message.content {
                        if let MessageContent::ToolRequest(tool_request) = content {
                            tool_requests.insert(tool_request.id.clone(), tool_request.clone());
                        }
                        let should_project = match content {
                            MessageContent::ToolRequest(_)
                            | MessageContent::ToolResponse(_)
                            | MessageContent::Thinking(_)
                            | MessageContent::ActionRequired(_) => true,
                            _ => is_user_visible,
                        };
                        if !should_project {
                            continue;
                        }
                        let _ = self
                            .handle_message_content(
                                content,
                                &projected_message,
                                &acp_session_id,
                                &agent,
                                &tool_requests,
                                &cx,
                            )
                            .await;
                    }
                    if is_visible_assistant {
                        let text = projected_message
                            .content
                            .iter()
                            .filter_map(MessageContent::as_text)
                            .collect::<String>();
                        if !text.trim().is_empty() {
                            if outcome_message_id.is_some()
                                && outcome_message_id == projected_message.id
                            {
                                outcome.push_str(&text);
                            } else {
                                outcome_message_id = projected_message.id;
                                outcome = text;
                            }
                        }
                    }
                }
                Ok(crate::agents::AgentEvent::McpNotification((request_id, notification))) => {
                    if let Some(update) =
                        tool_notifications::tool_notification_update(request_id, notification)
                    {
                        let _ = ToolCallNotifier::new(&cx, &acp_session_id).send_update(update);
                    }
                }
                Ok(crate::agents::AgentEvent::MessageUsage { message_id, usage }) => {
                    if self.supports_goose_custom_notifications() {
                        let _ = cx.send_notification(GooseSessionNotification {
                            session_id: session_id.clone(),
                            update: GooseSessionUpdate::MessageUsage(message_usage_update(
                                message_id, &usage,
                            )),
                        });
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    self.clear_active_run(&session_id, &run_id).await;
                    let _ = Self::send_active_run_update(&cx, &acp_session_id, None);
                    return "The coding task failed before it could complete.".into();
                }
            }
        }

        self.clear_active_run(&session_id, &run_id).await;
        let _ = Self::send_active_run_update(&cx, &acp_session_id, None);
        if cancel_token.is_cancelled() {
            return "The coding task was cancelled.".into();
        }
        if outcome.is_empty() {
            return "The coding task finished without a user-facing result.".into();
        }
        outcome
    }

    async fn steer_live_delegation(&self, session_id: &str, input: String) -> String {
        let Some((_, agent)) = self.active_runs.agent_run(session_id) else {
            return "The task could not receive the latest instruction.".into();
        };
        agent
            .steer(session_id, Message::user().with_text(input).agent_only())
            .await;
        "The latest instruction was added to the work in progress. Wait for its updated result."
            .into()
    }

    async fn prepare_live_agent(&self, session_id: &str) -> Result<Arc<Agent>, String> {
        if !crate::agents::state_machine::enabled() {
            return Err("Live coding requires the state machine.".into());
        }
        self.get_session_agent(session_id)
            .await
            .map_err(|_| "Goose could not activate the coding agent.".to_string())
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
