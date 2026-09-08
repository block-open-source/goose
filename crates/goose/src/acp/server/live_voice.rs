use super::*;
use crate::agents::live_voice::{LiveSessionId, LiveVoiceCoordinator};
use goose_providers::openai_live_voice_provider::OpenAiLiveVoiceProvider;

const MAX_SESSION_ID_BYTES: usize = 128;

pub struct LiveVoiceService {
    enabled: bool,
    provider_configured: bool,
    coordinator: LiveVoiceCoordinator,
}

impl LiveVoiceService {
    pub fn from_env() -> Self {
        let provider = Arc::new(OpenAiLiveVoiceProvider::from_env());
        Self::new(
            provider.is_enabled(),
            provider.is_configured(),
            LiveVoiceCoordinator::new(provider),
        )
    }

    pub fn new(
        enabled: bool,
        provider_configured: bool,
        coordinator: LiveVoiceCoordinator,
    ) -> Self {
        Self {
            enabled,
            provider_configured,
            coordinator,
        }
    }

    fn status(
        &self,
        session_id: &str,
        mode: GooseMode,
        prompt_active: bool,
    ) -> LiveVoiceAvailabilityResponse {
        use LiveVoiceStatus::*;

        let status = if !self.enabled {
            FeatureDisabled
        } else if !self.provider_configured {
            ProviderUnavailable
        } else if prompt_active {
            SessionBusy
        } else if mode != GooseMode::Auto {
            RequiresAutonomousMode
        } else if self
            .coordinator
            .has_live_session(&LiveSessionId(session_id.to_string()))
        {
            SessionBusy
        } else {
            Ready
        };

        LiveVoiceAvailabilityResponse { status }
    }
}

impl GooseAcpAgent {
    pub(super) async fn on_live_voice_availability(
        &self,
        req: LiveVoiceAvailabilityRequest,
    ) -> Result<LiveVoiceAvailabilityResponse, agent_client_protocol::Error> {
        if req.session_id.is_empty() || req.session_id.len() > MAX_SESSION_ID_BYTES {
            return Err(agent_client_protocol::Error::invalid_params());
        }
        if !self.sessions.lock().await.contains_key(&req.session_id) {
            return Err(agent_client_protocol::Error::resource_not_found(None)
                .data("Session is not accessible"));
        }

        let session = self
            .session_manager
            .get_session(&req.session_id, false)
            .await
            .map_err(|_| {
                agent_client_protocol::Error::resource_not_found(None)
                    .data("Session is not accessible")
            })?;
        if !matches!(session.session_type, SessionType::User | SessionType::Acp)
            || session.parent_session_id.is_some()
        {
            return Err(agent_client_protocol::Error::resource_not_found(None)
                .data("Session is not accessible"));
        }

        let prompt_active = self
            .active_prompt_runs
            .lock()
            .await
            .contains_key(&req.session_id);
        Ok(self
            .live_voice
            .status(&req.session_id, session.goose_mode, prompt_active))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::live_voice::{LiveOwnerToken, StartRequest, WorkSessionId};
    use goose_providers::live_voice_provider::{
        fake::provider_channel, LiveVoiceCapabilities, LiveVoiceConfig, LiveVoiceMediaRequest,
    };
    use tokio::sync::mpsc;

    fn service(enabled: bool, configured: bool) -> LiveVoiceService {
        let (provider, _starts) = provider_channel(LiveVoiceCapabilities {
            available: enabled && configured,
            ..Default::default()
        });
        LiveVoiceService::new(enabled, configured, LiveVoiceCoordinator::new(provider))
    }

    fn assert_status(
        service: &LiveVoiceService,
        mode: GooseMode,
        prompt_active: bool,
        expected: LiveVoiceStatus,
    ) {
        let response = service.status("main-session", mode, prompt_active);
        assert_eq!(response.status, expected);
    }

    #[test]
    fn reports_each_static_eligibility_gate() {
        assert_status(
            &service(false, true),
            GooseMode::Auto,
            false,
            LiveVoiceStatus::FeatureDisabled,
        );
        assert_status(
            &service(true, false),
            GooseMode::Auto,
            false,
            LiveVoiceStatus::ProviderUnavailable,
        );
        assert_status(
            &service(true, true),
            GooseMode::Auto,
            true,
            LiveVoiceStatus::SessionBusy,
        );
        assert_status(
            &service(true, true),
            GooseMode::Approve,
            false,
            LiveVoiceStatus::RequiresAutonomousMode,
        );
        assert_status(
            &service(true, true),
            GooseMode::Auto,
            false,
            LiveVoiceStatus::Ready,
        );
    }

    #[tokio::test]
    async fn reports_a_reserved_live_session_as_active() {
        let (provider, mut starts) = provider_channel(LiveVoiceCapabilities {
            available: true,
            ..Default::default()
        });
        let coordinator = LiveVoiceCoordinator::new(provider);
        let service = LiveVoiceService::new(true, true, coordinator.clone());
        let (updates, _update_rx) = mpsc::unbounded_channel();
        let start_coordinator = coordinator.clone();
        let start = tokio::spawn(async move {
            start_coordinator
                .start(StartRequest {
                    owner: LiveOwnerToken("owner".into()),
                    live_session_id: LiveSessionId("main-session".into()),
                    linked_work_session_id: Some(WorkSessionId("main-session".into())),
                    attempt: 1,
                    config: LiveVoiceConfig::default(),
                    media: LiveVoiceMediaRequest::WebRtc {
                        offer_sdp: "offer".into(),
                    },
                    updates,
                })
                .await
        });
        let pending_start = starts.recv().await.expect("provider start request");

        assert_status(
            &service,
            GooseMode::Auto,
            false,
            LiveVoiceStatus::SessionBusy,
        );

        pending_start.reject("done").unwrap();
        let _ = start.await;
    }
}
