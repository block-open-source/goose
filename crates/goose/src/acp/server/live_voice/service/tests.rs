
use super::*;
use goose_providers::live_voice_provider::fake::{
    provider_channel, provider_channel_with_availability, FakeConnectionDriver,
};
use goose_providers::model::ModelConfig;
use rmcp::model::Role;
use tokio::task::JoinHandle;

type StartResult = Result<StartLiveVoiceCallResult, LiveVoiceError>;

fn ignore_call_ended() -> LiveVoiceCallEndedHandler {
    Arc::new(|_| {})
}

fn ignore_transcript() -> LiveVoiceTranscriptHandler {
    Arc::new(|_| {})
}

fn ignore_delegation() -> LiveVoiceDelegationHandler {
    Arc::new(|_, _| Box::pin(async { "unused".to_string() }))
}

fn session_manager() -> Arc<SessionManager> {
    Arc::new(SessionManager::new(tempfile::tempdir().unwrap().keep()))
}

fn service(availability: LiveVoiceProviderAvailability) -> LiveVoiceService {
    let (provider, _starts) = provider_channel_with_availability(availability);
    LiveVoiceService::new(provider, Arc::new(ActiveRunRegistry::default()))
}

async fn live_session(
    conversation: impl IntoIterator<Item = Message>,
) -> (Arc<SessionManager>, String) {
    let manager = session_manager();
    let session = manager
        .create_session(
            std::path::PathBuf::from("/tmp/test"),
            "Live voice".into(),
            crate::session::session_manager::SessionType::User,
            GooseMode::Auto,
        )
        .await
        .unwrap();
    manager
        .update(&session.id)
        .provider_name("test")
        .model_config(ModelConfig::new("test-model"))
        .apply()
        .await
        .unwrap();
    for message in conversation {
        manager.add_message(&session.id, &message).await.unwrap();
    }
    (manager, session.id)
}

fn spawn_start(
    service: Arc<LiveVoiceService>,
    session_id: String,
    offer: &'static str,
    session_manager: Arc<SessionManager>,
) -> JoinHandle<StartResult> {
    tokio::spawn(async move {
        service
            .start_call(
                &session_id,
                WebRtcOffer::new(offer.into()).unwrap(),
                session_manager,
                ignore_transcript(),
                ignore_call_ended(),
                ignore_delegation(),
            )
            .await
    })
}

fn assert_availability(service: &LiveVoiceService, expected: LiveVoiceAvailability) {
    assert_eq!(
        service.availability("main-session", GooseMode::Auto),
        expected
    );
}

async fn establish_call() -> (
    Arc<LiveVoiceService>,
    FakeConnectionDriver,
    LiveVoiceCallId,
    String,
) {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::new(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (manager, session_id) = live_session([Message::user().with_text("prior context")]).await;
    let start_task = spawn_start(service.clone(), session_id.clone(), "offer", manager);
    let connection = starts
        .recv()
        .await
        .unwrap()
        .accept(WebRtcAnswer::new("answer".into()).unwrap())
        .unwrap();
    let call_id = start_task.await.unwrap().unwrap().call_id;
    (service, connection, call_id, session_id)
}

fn completion_receiver(
    service: &LiveVoiceService,
    session_id: &str,
) -> watch::Receiver<Option<LiveVoiceCallCompletion>> {
    service
        .calls_by_session
        .lock()
        .unwrap()
        .get(session_id)
        .unwrap()
        .completion_rx
        .clone()
}

#[test]
fn reports_each_eligibility_gate() {
    assert_availability(
        &service(LiveVoiceProviderAvailability::Disabled),
        LiveVoiceAvailability::FeatureDisabled,
    );
    assert_availability(
        &service(LiveVoiceProviderAvailability::Unavailable),
        LiveVoiceAvailability::ProviderUnavailable,
    );

    let ready = service(LiveVoiceProviderAvailability::Ready);
    let run_guard = LiveRunGuard::start(ready.active_runs.clone(), "main-session").unwrap();
    assert_eq!(
        ready.availability("main-session", GooseMode::Auto),
        LiveVoiceAvailability::ChatBusy
    );
    drop(run_guard);
    assert_eq!(
        ready.availability("main-session", GooseMode::Approve),
        LiveVoiceAvailability::RequiresAutonomousMode
    );
    assert_availability(&ready, LiveVoiceAvailability::Ready);
}

#[test]
fn input_messages_are_the_latest_visible_non_empty_text() {
    let mut messages = vec![
        Message::user().with_text("hidden").agent_only(),
        Message::assistant().with_thinking("internal", "signature"),
        Message::user().with_text(" "),
    ];
    messages.extend((0..12).map(|index| {
        if index % 2 == 0 {
            Message::user().with_text(format!("message {index}"))
        } else {
            Message::assistant().with_text(format!("message {index}"))
        }
    }));
    messages.push(Message::assistant().with_text("delegated result"));
    messages.push(Message::assistant().with_text("other hidden").agent_only());
    let conversation = Conversation::new_unvalidated(messages);

    let input_messages = live_voice_input_messages(&conversation);
    assert_eq!(input_messages.len(), LIVE_VOICE_INPUT_MESSAGE_COUNT);
    assert_eq!(input_messages.first().unwrap().text, "message 3");
    assert_eq!(input_messages.first().unwrap().role, Role::Assistant);
    assert_eq!(input_messages.last().unwrap().text, "delegated result");
    assert_eq!(input_messages.last().unwrap().role, Role::Assistant);
    assert!(!input_messages
        .iter()
        .any(|message| message.text == "other hidden"));
}

#[tokio::test]
async fn a_start_reserves_the_session_until_it_finishes() {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::new(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (manager, session_id) = live_session([Message::user().with_text("prior context")]).await;
    let first = spawn_start(
        service.clone(),
        session_id.clone(),
        "first-offer",
        manager.clone(),
    );
    let pending = starts.recv().await.unwrap();

    assert_eq!(
        pending.input_messages,
        vec![LiveVoiceInputMessage {
            role: Role::User,
            text: "prior context".into(),
        }]
    );

    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::ChatBusy
    );

    let second = service
        .start_call(
            &session_id,
            WebRtcOffer::new("second-offer".into()).unwrap(),
            manager,
            ignore_transcript(),
            ignore_call_ended(),
            ignore_delegation(),
        )
        .await;
    assert!(matches!(second, Err(LiveVoiceError::Unavailable)));

    pending.reject("failed").unwrap();
    assert!(matches!(
        first.await.unwrap(),
        Err(LiveVoiceError::StartFailed)
    ));
    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
}

#[tokio::test]
async fn a_cancelled_start_releases_the_session() {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::new(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (manager, session_id) = live_session([]).await;
    let start_task = spawn_start(service.clone(), session_id.clone(), "offer", manager);
    let pending = starts.recv().await.unwrap();

    start_task.abort();
    assert!(matches!(start_task.await, Err(error) if error.is_cancelled()));
    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
    drop(pending);
}

#[tokio::test]
async fn stop_failure_releases_the_session() {
    let (service, mut connection, call_id, session_id) = establish_call().await;
    let stop_service = service.clone();
    let stop_session_id = session_id.clone();
    let stop =
        tokio::spawn(async move { stop_service.stop_call(&stop_session_id, &call_id).await });
    connection
        .next_stop_request()
        .await
        .unwrap()
        .send(Err("failed".into()))
        .unwrap();

    assert!(matches!(
        stop.await.unwrap(),
        Err(LiveVoiceError::StopFailed)
    ));
    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
}

#[tokio::test]
async fn repeated_stop_uses_one_provider_shutdown() {
    let (service, mut connection, call_id, session_id) = establish_call().await;
    let first = service.stop_call(&session_id, &call_id);
    let second = service.stop_call(&session_id, &call_id);
    let provider = async move {
        connection
            .next_stop_request()
            .await
            .unwrap()
            .send(Ok(()))
            .unwrap();
        connection
            .send_event(ProviderConnectionEvent::Closed)
            .unwrap();
        assert!(connection.next_stop_request().await.is_none());
    };

    let (first, second, ()) = tokio::join!(first, second, provider);

    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
}

#[tokio::test]
async fn transcript_is_projected_live_and_persisted_after_stop_draining() {
    let temp_dir = tempfile::tempdir().unwrap();
    let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
    let session = session_manager
        .create_session(
            std::path::PathBuf::from("/tmp/test"),
            "Live transcript".into(),
            crate::session::session_manager::SessionType::User,
            GooseMode::Auto,
        )
        .await
        .unwrap();
    session_manager
        .update(&session.id)
        .provider_name("test")
        .model_config(ModelConfig::new("test-model"))
        .apply()
        .await
        .unwrap();
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::new(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (transcript_tx, mut transcript_rx) = tokio::sync::mpsc::unbounded_channel();
    let transcript_handler: LiveVoiceTranscriptHandler = Arc::new(move |message| {
        transcript_tx.send(message).unwrap();
    });
    let start_service = service.clone();
    let start_session_id = session.id.clone();
    let start_manager = session_manager.clone();
    let start = tokio::spawn(async move {
        start_service
            .start_call(
                &start_session_id,
                WebRtcOffer::new("offer".into()).unwrap(),
                start_manager,
                transcript_handler,
                ignore_call_ended(),
                ignore_delegation(),
            )
            .await
    });
    let mut connection = starts
        .recv()
        .await
        .unwrap()
        .accept(WebRtcAnswer::new("answer".into()).unwrap())
        .unwrap();
    let call_id = start.await.unwrap().unwrap().call_id;
    let delta = |event_id: &str, text: &str| ProviderConnectionEvent::TranscriptDelta {
        event_id: event_id.into(),
        role: Role::User,
        text: text.into(),
        start_ms: 0,
        end_ms: 1,
    };

    connection.send_event(delta("1", "hello")).unwrap();
    assert_eq!(
        transcript_rx.recv().await.unwrap().as_concat_text(),
        "hello"
    );
    connection
        .send_event(ProviderConnectionEvent::DelegationRequested {
            event_id: "delegation-event".into(),
            delegation_id: "delegation".into(),
            offset_ms: 1,
        })
        .unwrap();
    assert_eq!(
        connection.next_delegation_update().await.unwrap().text,
        "unused"
    );
    assert!(session_manager
        .get_session(&session.id, true)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages()
        .is_empty());

    let stop_service = service.clone();
    let stop_session_id = session.id.clone();
    let stop =
        tokio::spawn(async move { stop_service.stop_call(&stop_session_id, &call_id).await });
    let stop_response = connection.next_stop_request().await.unwrap();
    connection.send_event(delta("2", " world")).unwrap();
    stop_response.send(Ok(())).unwrap();
    connection
        .send_event(ProviderConnectionEvent::Closed)
        .unwrap();

    let revised = transcript_rx.recv().await.unwrap();
    assert_eq!(revised.as_concat_text(), " world");
    assert!(stop.await.unwrap().is_ok());

    let stored = session_manager
        .get_session(&session.id, true)
        .await
        .unwrap();
    let messages = stored.conversation.unwrap();
    assert_eq!(messages.messages().len(), 1);
    assert_eq!(messages.messages()[0].as_concat_text(), "hello world");
}

#[tokio::test]
async fn provider_terminal_events_fail_and_release_the_session() {
    for event in [
        ProviderConnectionEvent::Closed,
        ProviderConnectionEvent::Failed,
    ] {
        let (service, mut connection, call_id, session_id) = establish_call().await;
        let completion_rx = completion_receiver(&service, &session_id);
        let requires_cleanup = event == ProviderConnectionEvent::Failed;
        connection.send_event(event).unwrap();
        if requires_cleanup {
            connection
                .next_stop_request()
                .await
                .unwrap()
                .send(Ok(()))
                .unwrap();
        }

        assert_eq!(
            wait_for_completion(completion_rx).await.unwrap(),
            LiveVoiceCallCompletion::Failed
        );
        assert!(matches!(
            service.stop_call(&session_id, &call_id).await,
            Err(LiveVoiceError::Unavailable)
        ));
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
    }
}

#[tokio::test]
async fn provider_terminal_publishes_one_call_ended_update_after_release() {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::new(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (ended_tx, mut ended_rx) = tokio::sync::mpsc::unbounded_channel();
    let call_ended_handler: LiveVoiceCallEndedHandler = Arc::new(move |ended| {
        ended_tx.send(ended).unwrap();
    });
    let start_service = service.clone();
    let (manager, session_id) = live_session([]).await;
    let start_session_id = session_id.clone();
    let start_task = tokio::spawn(async move {
        start_service
            .start_call(
                &start_session_id,
                WebRtcOffer::new("offer".into()).unwrap(),
                manager,
                ignore_transcript(),
                call_ended_handler,
                ignore_delegation(),
            )
            .await
    });
    let connection = starts
        .recv()
        .await
        .unwrap()
        .accept(WebRtcAnswer::new("answer".into()).unwrap())
        .unwrap();
    let call_id = start_task.await.unwrap().unwrap().call_id;

    connection
        .send_event(ProviderConnectionEvent::Closed)
        .unwrap();
    let ended = ended_rx.recv().await.unwrap();

    assert_eq!(ended.session_id, session_id);
    assert_eq!(ended.call_id, call_id);
    assert_eq!(ended.completion, LiveVoiceCallCompletion::Failed);
    assert_eq!(
        service.availability(&ended.session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
    assert!(ended_rx.try_recv().is_err());
}

#[tokio::test]
async fn cleanup_timeout_fails_and_releases_the_session() {
    let (service, mut connection, call_id, session_id) = establish_call().await;
    tokio::time::pause();
    let stop_service = service.clone();
    let stop_session_id = session_id.clone();
    let stop =
        tokio::spawn(async move { stop_service.stop_call(&stop_session_id, &call_id).await });
    let pending_response = connection.next_stop_request().await.unwrap();
    tokio::time::advance(PROVIDER_CLEANUP_TIMEOUT).await;

    assert!(matches!(
        stop.await.unwrap(),
        Err(LiveVoiceError::StopFailed)
    ));
    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
    drop(pending_response);
}

#[tokio::test]
async fn queued_stop_wins_a_provider_close_race() {
    let (service, mut connection, call_id, session_id) = establish_call().await;
    connection
        .send_event(ProviderConnectionEvent::Closed)
        .unwrap();

    let stop = service.stop_call(&session_id, &call_id);
    let provider = async move {
        connection
            .next_stop_request()
            .await
            .unwrap()
            .send(Ok(()))
            .unwrap();
    };
    let (stop, ()) = tokio::join!(stop, provider);

    assert!(stop.is_ok());
    assert_eq!(
        service.availability(&session_id, GooseMode::Auto),
        LiveVoiceAvailability::Ready
    );
}

#[test]
fn stale_cleanup_cannot_remove_a_later_call() {
    let active_runs = Arc::new(ActiveRunRegistry::default());
    let calls = Arc::new(Mutex::new(HashMap::new()));
    let current_id = LiveVoiceCallId("current".into());
    let (_, completion_rx) = watch::channel(None);
    calls.lock().unwrap().insert(
        "main-session".into(),
        LiveCallControl {
            _run_guard: LiveRunGuard::start(active_runs.clone(), "main-session").unwrap(),
            call_id: current_id.clone(),
            stop_requested: CancellationToken::new(),
            completion_rx,
        },
    );

    remove_matching_call(&calls, "main-session", &LiveVoiceCallId("stale".into()));

    assert!(matches!(
        calls.lock().unwrap().get("main-session"),
        Some(control) if control.call_id == current_id
    ));
    assert!(active_runs.is_active("main-session"));
}
