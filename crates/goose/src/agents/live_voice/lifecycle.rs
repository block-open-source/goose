use super::{cleanup::cleanup, startup::ConnectionContext, types::*, update_sink::UpdateSink};
use goose_providers::live_voice_provider::{
    ProviderCommand, ProviderCommandId, ProviderConnection, ProviderDelegation,
    ProviderDelegationId, ProviderDelegationTarget, ProviderDispatchResult, ProviderEvent,
    ProviderEventStreamError,
};
use std::{
    collections::{HashSet, VecDeque},
    future::pending,
};
use tokio::time::{sleep_until, Instant};

const UNSUPPORTED_WORK: &str =
    "I can't start Goose work from Live voice yet. End Live and send the request in chat.";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ConnectionKey {
    pub owner: LiveOwnerToken,
    live_session_id: LiveSessionId,
}

impl ConnectionKey {
    pub fn new(owner: LiveOwnerToken, live_session_id: LiveSessionId) -> Self {
        Self {
            owner,
            live_session_id,
        }
    }
}

pub(super) enum LifecycleAction {
    Command(LiveVoiceCommand),
    Stop(LiveTerminalReason),
}

struct PendingInput {
    id: ProviderCommandId,
    expects_paused: bool,
    deadline: Instant,
}

struct PendingDelegation {
    id: ProviderCommandId,
    request: ProviderDelegation,
    deadline: Instant,
}

#[derive(Default)]
struct MediaReadiness {
    answer_applied: bool,
    remote_track_live: bool,
    peer_connected: bool,
    data_channel_open: bool,
}

impl MediaReadiness {
    fn observe(&mut self, observation: MediaReadinessObservation) {
        match observation {
            MediaReadinessObservation::AnswerApplied => self.answer_applied = true,
            MediaReadinessObservation::RemoteTrackLive => self.remote_track_live = true,
            MediaReadinessObservation::PeerConnected => self.peer_connected = true,
            MediaReadinessObservation::DataChannelOpen => self.data_channel_open = true,
        }
    }

    fn is_ready(&self) -> bool {
        self.answer_applied
            && self.remote_track_live
            && self.peer_connected
            && self.data_channel_open
    }
}

pub(super) struct ConnectionLifecycle<'a> {
    args: &'a mut ConnectionContext,
    updates: &'a UpdateSink,
    provider: ProviderConnection,
    media_readiness: MediaReadiness,
    provider_ready: bool,
    active: bool,
    muted_intent: bool,
    provider_input_enabled: bool,
    output_speaking: Option<bool>,
    pending_input: Option<PendingInput>,
    seen_delegations: HashSet<ProviderDelegationId>,
    delegations: VecDeque<ProviderDelegation>,
    pending_delegation: Option<PendingDelegation>,
    latest_usage: Option<goose_providers::live_voice_provider::ProviderUsage>,
    remote_acknowledged: bool,
    sequence: u64,
    readiness_deadline: Instant,
}

impl<'a> ConnectionLifecycle<'a> {
    pub fn new(
        args: &'a mut ConnectionContext,
        updates: &'a UpdateSink,
        provider: ProviderConnection,
    ) -> Self {
        Self {
            readiness_deadline: Instant::now() + args.inner.deadlines.readiness,
            args,
            updates,
            provider,
            media_readiness: MediaReadiness::default(),
            provider_ready: false,
            active: false,
            muted_intent: false,
            provider_input_enabled: false,
            output_speaking: None,
            pending_input: None,
            seen_delegations: HashSet::new(),
            delegations: VecDeque::new(),
            pending_delegation: None,
            latest_usage: None,
            remote_acknowledged: false,
            sequence: 0,
        }
    }

    pub async fn run(mut self) {
        let reason = loop {
            if let Some(reason) = self.dispatch_next().await {
                break reason;
            }
            let deadline = self.next_deadline();
            tokio::select! {
                // A queued user intent must win over a stale provider acknowledgement.
                biased;
                action = self.args.actions.recv() => {
                    if let Some(reason) = self.handle_action(action).await { break reason; }
                }
                event = self.provider.events.recv() => {
                    if let Some(reason) = self.handle_event(event) { break reason; }
                }
                _ = wait_for(deadline) => break self.timeout_reason(),
            }
        };

        let fallback = matches!(
            reason,
            LiveTerminalReason::ProviderFailed(_) | LiveTerminalReason::EventContinuityLost(_)
        );
        cleanup(
            self.args,
            self.updates,
            self.provider,
            reason,
            self.remote_acknowledged,
            self.latest_usage,
            fallback,
        )
        .await;
    }

    async fn dispatch_next(&mut self) -> Option<LiveTerminalReason> {
        let should_enable_provider_input = !self.muted_intent;
        if self.active
            && self.pending_input.is_none()
            && self.provider_input_enabled != should_enable_provider_input
        {
            let id = self.command_id("input");
            let command = if self.muted_intent {
                ProviderCommand::PauseInput {
                    command_id: id.clone(),
                }
            } else {
                ProviderCommand::ResumeInput {
                    command_id: id.clone(),
                }
            };
            self.pending_input = Some(PendingInput {
                id,
                expects_paused: self.muted_intent,
                deadline: Instant::now() + self.args.inner.deadlines.acknowledgement,
            });
            return self
                .dispatch(command)
                .await
                .err()
                .map(LiveTerminalReason::CommandFailed);
        }

        if self.pending_delegation.is_none() {
            if let Some(request) = self.delegations.pop_front() {
                let id = self.command_id("delegation");
                self.updates
                    .send(LiveVoiceEvent::DelegationPending(request.id.clone()));
                let command = ProviderCommand::DeliverDelegationResult {
                    command_id: id.clone(),
                    delegation_id: request.id.clone(),
                    result: UNSUPPORTED_WORK.into(),
                };
                self.pending_delegation = Some(PendingDelegation {
                    id,
                    request,
                    deadline: Instant::now() + self.args.inner.deadlines.acknowledgement,
                });
                if let Err(error) = self.dispatch(command).await {
                    let pending = self
                        .pending_delegation
                        .take()
                        .expect("delegation was just registered");
                    self.updates.send(LiveVoiceEvent::DelegationFailed(
                        pending.request.id,
                        error.clone(),
                    ));
                    return Some(LiveTerminalReason::CommandFailed(error));
                }
            }
        }
        None
    }

    async fn handle_action(
        &mut self,
        action: Option<LifecycleAction>,
    ) -> Option<LiveTerminalReason> {
        let command = match action {
            None => return Some(LiveTerminalReason::OwnerLost),
            Some(LifecycleAction::Command(command)) => command,
            Some(LifecycleAction::Stop(reason)) => return Some(reason),
        };
        match command {
            LiveVoiceCommand::ObserveMedia(observation) => {
                self.media_readiness.observe(observation);
                self.activate();
                None
            }
            LiveVoiceCommand::ObserveBootstrap(observation) => {
                let id = self.command_id("bootstrap");
                self.dispatch(ProviderCommand::ValidateStartupObservation {
                    command_id: id,
                    observation,
                })
                .await
                .err()
                .map(LiveTerminalReason::CommandFailed)
            }
            LiveVoiceCommand::SetMuted(muted) => {
                self.muted_intent = muted;
                self.emit_state();
                None
            }
            LiveVoiceCommand::MediaFailed(error) => Some(LiveTerminalReason::MediaFailed(error)),
            LiveVoiceCommand::MediaCleanupResult(_) => None,
        }
    }

    fn handle_event(
        &mut self,
        event: Option<Result<ProviderEvent, ProviderEventStreamError>>,
    ) -> Option<LiveTerminalReason> {
        match event {
            Some(Ok(ProviderEvent::Ready)) => {
                self.provider_ready = true;
                self.activate();
            }
            Some(Ok(ProviderEvent::TranscriptFragment(value))) => {
                self.updates.send(LiveVoiceEvent::Transcript(value));
            }
            Some(Ok(ProviderEvent::ProjectedTurn(value))) => {
                self.updates.send(LiveVoiceEvent::ProjectedTurn(value));
            }
            Some(Ok(ProviderEvent::OutputActivityChanged { active })) => {
                self.output_speaking = Some(active);
                self.emit_state();
            }
            Some(Ok(ProviderEvent::UsageUpdated(usage))) => self.latest_usage = Some(usage),
            Some(Ok(ProviderEvent::DelegationRequested(request))) => {
                if self.seen_delegations.insert(request.id.clone()) {
                    match request.target {
                        ProviderDelegationTarget::Client => self.delegations.push_back(request),
                        ProviderDelegationTarget::Unsupported(_) => self
                            .updates
                            .send(LiveVoiceEvent::DelegationUnsupportedTarget(request.id)),
                    }
                }
            }
            Some(Ok(ProviderEvent::InputPaused { command_id })) => {
                if self
                    .pending_input
                    .as_ref()
                    .is_some_and(|pending| pending.id == command_id && pending.expects_paused)
                {
                    self.pending_input = None;
                    self.provider_input_enabled = false;
                    self.emit_state();
                }
            }
            Some(Ok(ProviderEvent::InputResumed { command_id })) => {
                if self
                    .pending_input
                    .as_ref()
                    .is_some_and(|pending| pending.id == command_id && !pending.expects_paused)
                {
                    self.pending_input = None;
                    self.provider_input_enabled = true;
                    if !self.muted_intent {
                        self.emit_state();
                    }
                }
            }
            Some(Ok(ProviderEvent::DelegationResultAccepted {
                command_id,
                delegation_id,
            })) => {
                if self.pending_delegation.as_ref().is_some_and(|pending| {
                    pending.id == command_id && pending.request.id == delegation_id
                }) {
                    self.pending_delegation = None;
                    self.updates
                        .send(LiveVoiceEvent::DelegationDelivered(delegation_id));
                }
            }
            Some(Ok(ProviderEvent::CommandRejected { command_id, reason })) => {
                if let Some(pending) = self
                    .pending_delegation
                    .as_ref()
                    .filter(|pending| command_id.as_ref().is_some_and(|id| id == &pending.id))
                {
                    self.updates.send(LiveVoiceEvent::DelegationFailed(
                        pending.request.id.clone(),
                        reason.clone(),
                    ));
                }
                return Some(LiveTerminalReason::CommandFailed(reason));
            }
            Some(Ok(ProviderEvent::RemoteClosed { reason, usage })) => {
                self.remote_acknowledged = true;
                if usage.is_some() {
                    self.latest_usage = usage;
                }
                return Some(LiveTerminalReason::ProviderClosed(reason));
            }
            Some(Err(ProviderEventStreamError::Lagged(count))) => {
                return Some(LiveTerminalReason::EventContinuityLost(count));
            }
            Some(Err(ProviderEventStreamError::Failed(error))) => {
                return Some(LiveTerminalReason::ProviderFailed(error));
            }
            None => {
                return Some(LiveTerminalReason::ProviderFailed(
                    "event stream ended".into(),
                ))
            }
        }
        None
    }

    fn activate(&mut self) {
        if !self.active && self.provider_ready && self.media_readiness.is_ready() {
            self.active = true;
            self.emit_state();
        }
    }

    fn emit_state(&self) {
        self.updates.state(
            if self.active {
                LiveConnectionState::Active
            } else {
                LiveConnectionState::Connecting
            },
            self.muted_intent,
            self.provider_input_enabled,
            self.output_speaking,
        );
    }

    async fn dispatch(&self, command: ProviderCommand) -> Result<(), String> {
        match self.provider.control.dispatch(command).await {
            ProviderDispatchResult::Dispatched => Ok(()),
            ProviderDispatchResult::Rejected(error) => Err(error),
        }
    }

    fn command_id(&mut self, prefix: &str) -> ProviderCommandId {
        self.sequence += 1;
        ProviderCommandId(format!("{prefix}-{}", self.sequence))
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            (!self.active).then_some(self.readiness_deadline),
            self.pending_input.as_ref().map(|pending| pending.deadline),
            self.pending_delegation
                .as_ref()
                .map(|pending| pending.deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn timeout_reason(&self) -> LiveTerminalReason {
        if !self.active {
            return LiveTerminalReason::ReadinessTimedOut;
        }
        if let Some(pending) = &self.pending_delegation {
            self.updates.send(LiveVoiceEvent::DelegationFailed(
                pending.request.id.clone(),
                "result acknowledgement timed out".into(),
            ));
        }
        LiveTerminalReason::CommandFailed("provider acknowledgement timed out".into())
    }
}

async fn wait_for(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}
