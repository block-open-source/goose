use super::types::*;
use goose_providers::live_voice_provider::ProviderUsage;
use tokio::sync::mpsc;

pub(super) struct UpdateSink {
    sender: mpsc::UnboundedSender<LiveVoiceUpdate>,
    live_session_id: LiveSessionId,
    linked_work_session_id: Option<WorkSessionId>,
    attempt: StartAttempt,
    live_connection_id: LiveConnectionId,
}

impl UpdateSink {
    pub fn new(request: &StartRequest, live_connection_id: &LiveConnectionId) -> Self {
        Self {
            sender: request.updates.clone(),
            live_session_id: request.live_session_id.clone(),
            linked_work_session_id: request.linked_work_session_id.clone(),
            attempt: request.attempt,
            live_connection_id: live_connection_id.clone(),
        }
    }

    pub fn send(&self, event: LiveVoiceEvent) {
        let _ = self.sender.send(LiveVoiceUpdate {
            live_session_id: self.live_session_id.clone(),
            linked_work_session_id: self.linked_work_session_id.clone(),
            attempt: self.attempt,
            live_connection_id: self.live_connection_id.clone(),
            event,
        });
    }

    pub fn state(
        &self,
        connection: LiveConnectionState,
        muted_intent: bool,
        provider_input_enabled: bool,
        output_speaking: Option<bool>,
    ) {
        self.send(LiveVoiceEvent::State {
            connection,
            muted_intent,
            provider_input_enabled,
            output_speaking,
        });
    }

    pub fn terminal(
        &self,
        reason: LiveTerminalReason,
        remote_acknowledged: bool,
        provider_close_completed: bool,
        media_cleanup: Option<MediaCleanupResult>,
        usage: Option<ProviderUsage>,
    ) {
        self.send(LiveVoiceEvent::Terminal {
            reason,
            remote_acknowledged,
            provider_close_completed,
            media_cleanup,
            usage,
        });
    }
}
