mod call;
mod service;

use super::*;
use call::{LiveVoiceCallId, LiveVoiceCallState};
use service::{
    wait_until_finished, LiveVoiceAvailability, LiveVoiceCallEndedHandler, LiveVoiceError,
    LiveVoiceTranscriptHandler, WebRtcOffer,
};

pub use service::LiveVoiceService;

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
        &self,
        cx: &ConnectionTo<Client>,
        req: LiveVoiceStartRequest,
    ) -> Result<LiveVoiceStartResponse, agent_client_protocol::Error> {
        let offer = WebRtcOffer::new(req.offer_sdp)
            .ok_or_else(agent_client_protocol::Error::invalid_params)?;
        let session = self.load_live_voice_session(&req.session_id).await?;
        if session.provider_name.is_none() || session.model_config.is_none() {
            return Err(map_live_voice_error(LiveVoiceError::Unavailable));
        }

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
        let start = self.live_voice.start_call(
            &req.session_id,
            session.goose_mode,
            offer,
            self.session_manager.clone(),
            transcript_handler,
            call_ended_handler,
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
