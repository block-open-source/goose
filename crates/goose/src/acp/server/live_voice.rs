mod call;
mod service;

use super::*;
use call::LiveVoiceCallId;
use service::{LiveVoiceAvailability, LiveVoiceError, WebRtcOffer};

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
        req: LiveVoiceStartRequest,
    ) -> Result<LiveVoiceStartResponse, agent_client_protocol::Error> {
        let offer = WebRtcOffer::new(req.offer_sdp)
            .ok_or_else(agent_client_protocol::Error::invalid_params)?;
        let session = self.load_live_voice_session(&req.session_id).await?;
        if session.provider_name.is_none() || session.model_config.is_none() {
            return Err(map_live_voice_error(LiveVoiceError::Unavailable));
        }
        let call = self
            .live_voice
            .start_call(&req.session_id, session.goose_mode, offer)
            .await
            .map_err(map_live_voice_error)?;

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
