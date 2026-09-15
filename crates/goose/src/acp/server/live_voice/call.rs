mod transcript;

use super::service::{
    remove_call_if_current, LiveCallControls, LiveCallGuard, LiveVoiceCallCompletion,
    LiveVoiceTranscriptPublisher,
};
use crate::{conversation::message::Message, session::SessionManager, token_counter::TokenCounter};
use futures::future::BoxFuture;
use goose_providers::live_voice_provider::{ProviderConnection, ProviderConnectionEvent};
use rmcp::model::Role;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{sync::watch, time::timeout};
use tokio_util::sync::CancellationToken;
use transcript::{DelegationContext, LiveTranscript};
use uuid::Uuid;

pub(super) const PROVIDER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const DELEGATION_INSTRUCTION: &str =
    "Based on this conversation, identify and complete the user's request.";
const DELEGATION_UPDATE_TOKEN_LIMIT: usize = 500;
const SAVED_RESULT_NOTICE: &str = "\n\nThe full result is saved in Goose.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiveVoiceCallId(pub(super) String);

impl LiveVoiceCallId {
    pub(super) fn new() -> Self {
        Self(format!("live_{}", Uuid::now_v7()))
    }
}

pub(super) struct LiveVoiceCall {
    session_id: String,
    id: LiveVoiceCallId,
    provider_connection: Box<dyn ProviderConnection>,
    provider_event_ids: HashSet<String>,
    provider_delegation_ids: HashSet<String>,
    transcript: LiveTranscript,
    delegated_main_agent_run: Option<DelegatedMainAgentRun>,
}

struct DelegatedMainAgentRun {
    // GPT-Live expects the final result on the delegation that started this run.
    provider_delegation_id: String,
    result_future: BoxFuture<'static, String>,
}

enum LiveCallEvent {
    StopRequested,
    MainAgentFinished(String),
    Provider(ProviderConnectionEvent),
}

enum DelegationDecision {
    Ignore,
    Reject(String),
    Accept(String),
}

impl LiveVoiceCall {
    pub(super) fn new(
        session_id: String,
        id: LiveVoiceCallId,
        provider_connection: Box<dyn ProviderConnection>,
    ) -> Self {
        Self {
            session_id,
            id,
            provider_connection,
            provider_event_ids: HashSet::new(),
            provider_delegation_ids: HashSet::new(),
            transcript: LiveTranscript::default(),
            delegated_main_agent_run: None,
        }
    }

    pub(super) async fn run(mut self, runtime: LiveCallRuntime) {
        let mut stopping = false;
        let completion = loop {
            match self.next_event(&runtime.stop_requested, stopping).await {
                LiveCallEvent::StopRequested => {
                    match timeout(PROVIDER_CLEANUP_TIMEOUT, self.cleanup_provider()).await {
                        Ok(Ok(())) => stopping = true,
                        _ => break LiveVoiceCallCompletion::Failed,
                    }
                }
                LiveCallEvent::MainAgentFinished(result) => {
                    if self
                        .handle_main_agent_finished(&runtime, result)
                        .await
                        .is_err()
                    {
                        self.stop_provider_after_error(stopping).await;
                        break LiveVoiceCallCompletion::Failed;
                    }
                }
                LiveCallEvent::Provider(ProviderConnectionEvent::TranscriptDelta {
                    event_id,
                    role,
                    text,
                    end_ms,
                    ..
                }) => {
                    if self
                        .receive_transcript(&runtime, event_id, role, text, end_ms)
                        .await
                        .is_err()
                    {
                        self.stop_provider_after_error(stopping).await;
                        break LiveVoiceCallCompletion::Failed;
                    }
                }
                LiveCallEvent::Provider(ProviderConnectionEvent::DelegationRequested {
                    event_id,
                    delegation_id,
                    offset_ms,
                }) => {
                    if self
                        .receive_delegation(&runtime, event_id, delegation_id, offset_ms)
                        .await
                        .is_err()
                    {
                        self.stop_provider_after_error(stopping).await;
                        break LiveVoiceCallCompletion::Failed;
                    }
                }
                LiveCallEvent::Provider(ProviderConnectionEvent::Closed) => {
                    break if stopping {
                        LiveVoiceCallCompletion::Stopped
                    } else {
                        LiveVoiceCallCompletion::Failed
                    };
                }
                LiveCallEvent::Provider(
                    ProviderConnectionEvent::ReceiverLagged | ProviderConnectionEvent::Failed,
                ) => {
                    self.stop_provider_after_error(stopping).await;
                    break LiveVoiceCallCompletion::Failed;
                }
            }
        };

        self.finish_call(runtime, completion).await;
    }

    async fn next_event(
        &mut self,
        stop_requested: &CancellationToken,
        stopping: bool,
    ) -> LiveCallEvent {
        if stopping {
            return LiveCallEvent::Provider(self.provider_connection.next_event().await);
        }

        let delegated_run = &mut self.delegated_main_agent_run;
        let provider_connection = &mut self.provider_connection;
        tokio::select! {
            biased;
            _ = stop_requested.cancelled() => LiveCallEvent::StopRequested,
            result = async {
                delegated_run
                    .as_mut()
                    .expect("main agent is running")
                    .result_future
                    .as_mut()
                    .await
            }, if delegated_run.is_some() => LiveCallEvent::MainAgentFinished(result),
            event = provider_connection.next_event() => LiveCallEvent::Provider(event),
        }
    }

    async fn receive_transcript(
        &mut self,
        runtime: &LiveCallRuntime,
        event_id: String,
        role: Role,
        text: String,
        end_ms: u64,
    ) -> anyhow::Result<()> {
        let Some(transcript_delta_to_display) =
            self.record_transcript(event_id, role, &text, end_ms)
        else {
            return Ok(());
        };

        (runtime.transcript_publisher)(transcript_delta_to_display);
        save_raw_transcript_entries(
            &runtime.session_manager,
            &self.session_id,
            &mut self.transcript,
        )
        .await?;
        Ok(())
    }

    fn record_transcript(
        &mut self,
        event_id: String,
        role: Role,
        delta_text: &str,
        end_ms: u64,
    ) -> Option<Message> {
        if !self.provider_event_ids.insert(event_id) {
            return None;
        }
        self.transcript.append(role, delta_text, end_ms)
    }

    async fn receive_delegation(
        &mut self,
        runtime: &LiveCallRuntime,
        event_id: String,
        delegation_id: String,
        offset_ms: u64,
    ) -> anyhow::Result<()> {
        match self.handle_delegation_request(event_id, delegation_id.clone(), offset_ms) {
            DelegationDecision::Ignore => {}
            DelegationDecision::Reject(text) => {
                self.send_delegation_update(delegation_id, bound_delegation_update(text).await)
                    .await?;
            }
            DelegationDecision::Accept(context) => {
                if self.delegated_main_agent_run.is_some() {
                    let input = format!("{context}\n{DELEGATION_INSTRUCTION}");
                    let response = match runtime
                        .main_agent
                        .steer(self.session_id.clone(), input)
                        .await
                    {
                        Ok(response) => {
                            self.transcript
                                .mark_context_sent_to_main_agent_through(offset_ms);
                            response
                        }
                        Err(response) => response,
                    };
                    self.send_delegation_update(
                        delegation_id,
                        bound_delegation_update(response).await,
                    )
                    .await?;
                } else {
                    self.transcript.finish_transcript_entry_being_built();
                    save_raw_transcript_entries(
                        &runtime.session_manager,
                        &self.session_id,
                        &mut self.transcript,
                    )
                    .await?;
                    let input = format!("{context}\n{DELEGATION_INSTRUCTION}");
                    match runtime.main_agent.start(self.session_id.clone(), input) {
                        Ok(completion) => {
                            self.delegated_main_agent_run = Some(DelegatedMainAgentRun {
                                provider_delegation_id: delegation_id,
                                result_future: completion,
                            });
                            self.transcript
                                .mark_context_sent_to_main_agent_through(offset_ms);
                        }
                        Err(response) => {
                            self.send_delegation_update(
                                delegation_id,
                                bound_delegation_update(response).await,
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_delegation_request(
        &mut self,
        event_id: String,
        delegation_id: String,
        offset_ms: u64,
    ) -> DelegationDecision {
        if !self.provider_event_ids.insert(event_id)
            || !self.provider_delegation_ids.insert(delegation_id)
        {
            return DelegationDecision::Ignore;
        }
        match self.transcript.context_for_delegation(offset_ms) {
            DelegationContext::Stale => {
                DelegationDecision::Reject("The delegated conversation position is stale.".into())
            }
            DelegationContext::MissingUserInput => {
                DelegationDecision::Reject("I couldn't identify a request to complete.".into())
            }
            DelegationContext::Available(context) => DelegationDecision::Accept(context),
        }
    }

    async fn handle_main_agent_finished(
        &mut self,
        runtime: &LiveCallRuntime,
        result: String,
    ) -> anyhow::Result<()> {
        let run = self
            .delegated_main_agent_run
            .take()
            .expect("main agent is running");
        save_transcript_and_context_waiting_for_main_agent(
            &runtime.session_manager,
            &self.session_id,
            &mut self.transcript,
        )
        .await?;
        self.send_delegation_update(
            run.provider_delegation_id,
            bound_delegation_update(result).await,
        )
        .await
    }

    async fn finish_call(
        mut self,
        runtime: LiveCallRuntime,
        mut completion: LiveVoiceCallCompletion,
    ) {
        match self.delegated_main_agent_run.take() {
            None => {
                if save_transcript_and_context_waiting_for_main_agent(
                    &runtime.session_manager,
                    &self.session_id,
                    &mut self.transcript,
                )
                .await
                .is_err()
                {
                    completion = LiveVoiceCallCompletion::Failed;
                }
                remove_call_if_current(&runtime.calls_by_session, &self.session_id, &self.id);
                drop(runtime.call_guard);
                runtime.completion_tx.send_replace(Some(completion));
            }
            Some(run) => {
                remove_call_if_current(&runtime.calls_by_session, &self.session_id, &self.id);
                runtime.completion_tx.send_replace(Some(completion));
                let _ = run.result_future.await;
                let _ = save_transcript_and_context_waiting_for_main_agent(
                    &runtime.session_manager,
                    &self.session_id,
                    &mut self.transcript,
                )
                .await;
                drop(runtime.call_guard);
            }
        }
    }

    async fn stop_provider_after_error(&mut self, stopping: bool) {
        if !stopping {
            let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, self.cleanup_provider()).await;
        }
    }

    async fn send_delegation_update(
        &mut self,
        provider_delegation_id: String,
        text: String,
    ) -> anyhow::Result<()> {
        self.provider_connection
            .send_delegation_update(goose_providers::live_voice_provider::DelegationUpdate {
                provider_delegation_id,
                text,
            })
            .await
    }

    async fn cleanup_provider(&mut self) -> anyhow::Result<()> {
        self.provider_connection.stop().await
    }
}

// Server-owned handles used for the lifetime of one call loop.
pub(super) struct LiveCallRuntime {
    calls_by_session: LiveCallControls,
    stop_requested: CancellationToken,
    completion_tx: watch::Sender<Option<LiveVoiceCallCompletion>>,
    session_manager: Arc<SessionManager>,
    transcript_publisher: LiveVoiceTranscriptPublisher,
    main_agent: LiveMainAgent,
    call_guard: LiveCallGuard,
}

impl LiveCallRuntime {
    pub(super) fn new(
        calls_by_session: LiveCallControls,
        stop_requested: CancellationToken,
        completion_tx: watch::Sender<Option<LiveVoiceCallCompletion>>,
        session_manager: Arc<SessionManager>,
        transcript_publisher: LiveVoiceTranscriptPublisher,
        main_agent: LiveMainAgent,
        call_guard: LiveCallGuard,
    ) -> Self {
        Self {
            calls_by_session,
            stop_requested,
            completion_tx,
            session_manager,
            transcript_publisher,
            main_agent,
            call_guard,
        }
    }
}

pub(super) struct LiveMainAgent {
    start: Box<dyn Fn(String, String) -> Result<MainAgentRun, String> + Send + Sync>,
    steer: Box<dyn Fn(String, String) -> MainAgentSteerResponse + Send + Sync>,
}

type MainAgentRun = BoxFuture<'static, String>;
type MainAgentSteerResponse = BoxFuture<'static, Result<String, String>>;

impl LiveMainAgent {
    pub(super) fn new(
        start: impl Fn(String, String) -> Result<MainAgentRun, String> + Send + Sync + 'static,
        steer: impl Fn(String, String) -> MainAgentSteerResponse + Send + Sync + 'static,
    ) -> Self {
        Self {
            start: Box::new(start),
            steer: Box::new(steer),
        }
    }

    fn start(&self, session_id: String, input: String) -> Result<MainAgentRun, String> {
        (self.start)(session_id, input)
    }

    fn steer(&self, session_id: String, input: String) -> MainAgentSteerResponse {
        (self.steer)(session_id, input)
    }
}

async fn save_raw_transcript_entries(
    session_manager: &SessionManager,
    session_id: &str,
    transcript: &mut LiveTranscript,
) -> anyhow::Result<()> {
    for message in transcript.raw_transcript_entries_waiting_to_save() {
        session_manager.add_message(session_id, message).await?;
    }
    transcript.mark_raw_transcript_entries_saved();
    Ok(())
}

async fn save_transcript_and_context_waiting_for_main_agent(
    session_manager: &SessionManager,
    session_id: &str,
    transcript: &mut LiveTranscript,
) -> anyhow::Result<()> {
    transcript.finish_transcript_entry_being_built();
    save_raw_transcript_entries(session_manager, session_id, transcript).await?;
    if let Some(context) = transcript.agent_only_context_message_waiting_to_save() {
        session_manager.add_message(session_id, &context).await?;
        transcript.mark_context_saved_for_main_agent();
    }
    Ok(())
}

async fn bound_delegation_update(text: String) -> String {
    let Ok(counter) = TokenCounter::new().await else {
        return text;
    };
    if counter.count_tokens(&text) <= DELEGATION_UPDATE_TOKEN_LIMIT {
        return text;
    }

    let allowed = DELEGATION_UPDATE_TOKEN_LIMIT - counter.count_tokens(SAVED_RESULT_NOTICE);
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut low = 0;
    let mut high = boundaries.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let end = boundaries.get(middle).copied().unwrap_or(text.len());
        let candidate = text
            .get(..end)
            .expect("delegation update boundary comes from char_indices");
        if counter.count_tokens(candidate) <= allowed {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let end = boundaries.get(low).copied().unwrap_or(text.len());
    let truncated = text
        .get(..end)
        .expect("delegation update boundary comes from char_indices");
    format!("{}{}", truncated.trim_end(), SAVED_RESULT_NOTICE)
}

#[cfg(test)]
mod tests;
