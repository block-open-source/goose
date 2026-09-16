mod fake_live_voice_provider;

use super::super::call::{LiveMainAgent, DELEGATION_INSTRUCTION, PROVIDER_CLEANUP_TIMEOUT};
use super::*;
use fake_live_voice_provider::{provider_channel, FakeConnectionDriver};
use goose_providers::{live_voice_provider::ProviderConnectionEvent, model::ModelConfig};
use rmcp::model::Role;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

type StartResult = Result<StartLiveVoiceCallResult, LiveVoiceError>;

impl LiveVoiceService {
    fn for_test(provider: Arc<dyn LiveVoiceProvider>, active_runs: Arc<ActiveRunRegistry>) -> Self {
        Self::new(Arc::new(move || Ok(provider.clone())), active_runs)
    }
}

fn ignore_transcript_publisher() -> LiveVoiceTranscriptPublisher {
    Arc::new(|_| {})
}

fn ignore_main_agent() -> LiveMainAgent {
    LiveMainAgent::new(
        |_, _| Ok(Box::pin(async { "unused".to_string() })),
        |_, _| Box::pin(async { Ok("unused".to_string()) }),
    )
}

fn controlled_main_agent() -> (
    LiveMainAgent,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    oneshot::Sender<String>,
) {
    let (start_tx, start_rx) = tokio::sync::mpsc::unbounded_channel();
    let (steer_tx, steer_rx) = tokio::sync::mpsc::unbounded_channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let finish_rx = Arc::new(Mutex::new(Some(finish_rx)));
    let main_agent = LiveMainAgent::new(
        move |_, input| {
            start_tx.send(input).unwrap();
            let finish_rx = finish_rx
                .lock()
                .unwrap()
                .take()
                .expect("one delegation run");
            Ok(Box::pin(async move { finish_rx.await.unwrap() }))
        },
        move |_, input| {
            steer_tx.send(input).unwrap();
            Box::pin(async { Ok("steered".to_string()) })
        },
    );
    (main_agent, start_rx, steer_rx, finish_tx)
}

fn session_manager() -> Arc<SessionManager> {
    Arc::new(SessionManager::new(tempfile::tempdir().unwrap().keep()))
}

fn service() -> LiveVoiceService {
    let (provider, _starts) = provider_channel();
    LiveVoiceService::for_test(provider, Arc::new(ActiveRunRegistry::default()))
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
                ignore_transcript_publisher(),
                ignore_main_agent(),
            )
            .await
    })
}

fn assert_availability(service: &LiveVoiceService, expected: Result<(), &'static str>) {
    assert_eq!(
        service.availability(Some("main-session"), GooseMode::Auto),
        expected
    );
}

async fn establish_call() -> (
    Arc<LiveVoiceService>,
    FakeConnectionDriver,
    LiveVoiceCallId,
    String,
) {
    let (service, connection, call_id, session_id, _) =
        establish_call_with(ignore_main_agent(), ignore_transcript_publisher()).await;
    (service, connection, call_id, session_id)
}

async fn establish_call_with(
    main_agent: LiveMainAgent,
    transcript_publisher: LiveVoiceTranscriptPublisher,
) -> (
    Arc<LiveVoiceService>,
    FakeConnectionDriver,
    LiveVoiceCallId,
    String,
    Arc<SessionManager>,
) {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::for_test(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (manager, session_id) = live_session([Message::user().with_text("prior context")]).await;
    let start_service = service.clone();
    let start_session_id = session_id.clone();
    let start_manager = manager.clone();
    let start_task = tokio::spawn(async move {
        start_service
            .start_call(
                &start_session_id,
                WebRtcOffer::new("offer".into()).unwrap(),
                start_manager,
                transcript_publisher,
                main_agent,
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
    (service, connection, call_id, session_id, manager)
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
    let mut disabled = service();
    disabled.live_voice_resolver = Arc::new(|| Err("Live voice is disabled"));
    assert_availability(&disabled, Err("Live voice is disabled"));
    let mut unavailable = service();
    unavailable.live_voice_resolver = Arc::new(|| Err("Live voice provider is not configured"));
    assert_availability(&unavailable, Err("Live voice provider is not configured"));
    let ready = service();
    let call_guard = LiveCallGuard::start(ready.active_runs.clone(), "main-session").unwrap();
    assert_eq!(
        ready.availability(Some("main-session"), GooseMode::Auto),
        Err("Live voice is unavailable while this session is busy")
    );
    assert_eq!(ready.availability(None, GooseMode::Auto), Ok(()));
    drop(call_guard);
    assert_eq!(
        ready.availability(Some("main-session"), GooseMode::Approve),
        Err("Live voice requires Autonomous mode")
    );
    assert_availability(&ready, Ok(()));
}

#[test]
fn configured_live_voice_requires_the_state_machine() {
    let _guard = env_lock::lock_env([
        ("GOOSE_LIVE_VOICE_ENABLED", Some("1")),
        ("GOOSE_STATE_MACHINE", None::<&str>),
    ]);

    assert!(matches!(
        configured_live_voice(),
        Err("Live voice is disabled")
    ));
}

#[tokio::test]
async fn start_rechecks_requirements_before_contacting_the_provider() {
    let (provider, mut starts) = provider_channel();
    let mut service = LiveVoiceService::for_test(provider, Arc::new(ActiveRunRegistry::default()));
    service.live_voice_resolver = Arc::new(|| Err("Live voice is disabled"));
    let (manager, session_id) = live_session([]).await;

    let result = service
        .start_call(
            &session_id,
            WebRtcOffer::new("offer".into()).unwrap(),
            manager,
            ignore_transcript_publisher(),
            ignore_main_agent(),
        )
        .await;

    assert!(matches!(result, Err(LiveVoiceError::Unavailable)));
    assert!(starts.try_recv().is_err());
}

#[tokio::test]
async fn input_messages_include_all_visible_non_empty_text_when_it_fits() {
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

    let input_messages = live_voice_input_messages(&conversation).await.unwrap();
    assert_eq!(input_messages.len(), 13);
    assert_eq!(input_messages.first().unwrap().text, "message 0");
    assert_eq!(input_messages.first().unwrap().role, Role::User);
    assert_eq!(input_messages.last().unwrap().text, "delegated result");
    assert_eq!(input_messages.last().unwrap().role, Role::Assistant);
    assert!(!input_messages
        .iter()
        .any(|message| message.text == "other hidden"));
}

#[tokio::test]
async fn input_messages_keep_the_newest_complete_messages_within_provider_limits() {
    let message_limited = Conversation::new_unvalidated(
        (0..LIVE_VOICE_INPUT_MESSAGE_LIMIT + 2)
            .map(|index| Message::user().with_text(format!("message {index}"))),
    );
    let selected = live_voice_input_messages(&message_limited).await.unwrap();
    assert_eq!(selected.len(), LIVE_VOICE_INPUT_MESSAGE_LIMIT);
    assert_eq!(selected.first().unwrap().text, "message 2");
    assert_eq!(
        selected.last().unwrap().text,
        format!("message {}", LIVE_VOICE_INPUT_MESSAGE_LIMIT + 1)
    );

    let token_limited = Conversation::new_unvalidated([
        Message::user().with_text("earlier"),
        Message::assistant().with_text("history ".repeat(LIVE_VOICE_INPUT_TOKEN_LIMIT + 1)),
        Message::user().with_text("newest"),
    ]);
    assert_eq!(
        live_voice_input_messages(&token_limited).await.unwrap(),
        vec![LiveVoiceInputMessage {
            role: Role::User,
            text: "newest".into(),
        }]
    );
}

#[tokio::test]
async fn a_start_reserves_the_session_until_it_finishes() {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::for_test(
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
        service.availability(Some(&session_id), GooseMode::Auto),
        Err("Live voice is unavailable while this session is busy")
    );

    let second = service
        .start_call(
            &session_id,
            WebRtcOffer::new("second-offer".into()).unwrap(),
            manager,
            ignore_transcript_publisher(),
            ignore_main_agent(),
        )
        .await;
    assert!(matches!(second, Err(LiveVoiceError::Unavailable)));

    pending.reject("failed").unwrap();
    assert!(matches!(
        first.await.unwrap(),
        Err(LiveVoiceError::StartFailed)
    ));
    assert_eq!(
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
    );
}

#[tokio::test]
async fn a_cancelled_start_releases_the_session() {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::for_test(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (manager, session_id) = live_session([]).await;
    let start_task = spawn_start(service.clone(), session_id.clone(), "offer", manager);
    let pending = starts.recv().await.unwrap();

    start_task.abort();
    assert!(matches!(start_task.await, Err(error) if error.is_cancelled()));
    assert_eq!(
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
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
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
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
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
    );
}

#[tokio::test]
async fn transcript_without_delegation_saves_raw_and_main_agent_context_after_stop() {
    let (service, mut connection, call_id, session_id, manager) =
        establish_call_with(ignore_main_agent(), ignore_transcript_publisher()).await;
    connection
        .send_event(ProviderConnectionEvent::TranscriptDelta {
            event_id: "transcript".into(),
            role: Role::User,
            text: "spoken context".into(),
            start_ms: 0,
            end_ms: 1,
        })
        .unwrap();

    let stop = service.stop_call(&session_id, &call_id);
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
    };
    let (stop, ()) = tokio::join!(stop, provider);
    assert!(stop.is_ok());

    let session = manager.get_session(&session_id, true).await.unwrap();
    let conversation = session.conversation.unwrap();
    let transcript = conversation
        .messages()
        .iter()
        .find(|message| message.as_concat_text() == "spoken context")
        .cloned()
        .expect("Live transcript should be persisted");
    assert!(transcript.is_user_visible());
    assert!(!transcript.is_agent_visible());
    let context = conversation
        .messages()
        .iter()
        .find(|message| message.as_concat_text().contains("User: spoken context"))
        .expect("Live context should be persisted");
    assert!(!context.is_user_visible());
    assert!(context.is_agent_visible());
}

#[tokio::test]
async fn rejected_delegation_keeps_context_for_live_completion() {
    let main_agent = LiveMainAgent::new(
        |_, _| Err("main agent unavailable".into()),
        |_, _| Box::pin(async { Ok("unused".into()) }),
    );
    let (service, mut connection, call_id, session_id, manager) =
        establish_call_with(main_agent, ignore_transcript_publisher()).await;
    connection
        .send_event(ProviderConnectionEvent::TranscriptDelta {
            event_id: "transcript".into(),
            role: Role::User,
            text: "keep this context".into(),
            start_ms: 0,
            end_ms: 1,
        })
        .unwrap();
    connection
        .send_event(ProviderConnectionEvent::DelegationRequested {
            event_id: "delegation-event".into(),
            delegation_id: "delegation".into(),
            offset_ms: 1,
        })
        .unwrap();
    assert_eq!(
        connection.next_delegation_update().await.unwrap().text,
        "main agent unavailable"
    );

    let stop = service.stop_call(&session_id, &call_id);
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
    };
    let (stop, ()) = tokio::join!(stop, provider);
    assert!(stop.is_ok());

    let session = manager.get_session(&session_id, true).await.unwrap();
    let conversation = session.conversation.unwrap();
    assert!(conversation.messages().iter().any(|message| {
        message.is_agent_visible()
            && !message.is_user_visible()
            && message.as_concat_text().contains("User: keep this context")
    }));
}

#[tokio::test]
async fn transcript_is_projected_and_flushed_before_delegation() {
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
    let service = Arc::new(LiveVoiceService::for_test(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let (transcript_tx, mut transcript_rx) = tokio::sync::mpsc::unbounded_channel();
    let transcript_publisher: LiveVoiceTranscriptPublisher = Arc::new(move |message| {
        transcript_tx.send(message).unwrap();
    });
    let (start_tx, mut start_rx) = tokio::sync::mpsc::unbounded_channel();
    let main_agent = LiveMainAgent::new(
        move |_, input| {
            start_tx.send(input).unwrap();
            Ok(Box::pin(async { "unused".to_string() }))
        },
        |_, _| Box::pin(async { Ok("unused".to_string()) }),
    );
    let start_service = service.clone();
    let start_session_id = session.id.clone();
    let start_manager = session_manager.clone();
    let start = tokio::spawn(async move {
        start_service
            .start_call(
                &start_session_id,
                WebRtcOffer::new("offer".into()).unwrap(),
                start_manager,
                transcript_publisher,
                main_agent,
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
    let input = start_rx.recv().await.unwrap();
    assert_eq!(
        input,
        format!("Live conversation context:\nUser: hello\n{DELEGATION_INSTRUCTION}")
    );
    assert_eq!(
        connection.next_delegation_update().await.unwrap().text,
        "unused"
    );
    let stored = session_manager
        .get_session(&session.id, true)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(stored.messages().len(), 1);
    assert_eq!(stored.messages()[0].as_concat_text(), "hello");
    assert!(!stored.messages()[0].is_agent_visible());

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
    assert_eq!(messages.messages().len(), 3);
    assert_eq!(messages.messages()[0].as_concat_text(), "hello");
    assert_eq!(messages.messages()[1].as_concat_text(), " world");
    assert!(!messages.messages()[0].is_agent_visible());
    assert!(!messages.messages()[1].is_agent_visible());
    assert!(messages.messages()[2].is_agent_visible());
    assert!(!messages.messages()[2].is_user_visible());
    assert!(messages.messages()[2]
        .as_concat_text()
        .contains("User: world"));
    assert!(!messages.messages()[2].as_concat_text().contains("hello"));
}

#[tokio::test]
async fn running_transcript_is_saved_user_only_and_steered_to_the_main_agent() {
    let (main_agent, mut starts, mut steers, finish_run) = controlled_main_agent();
    let (transcript_tx, mut transcript_rx) = tokio::sync::mpsc::unbounded_channel();
    let transcript_publisher: LiveVoiceTranscriptPublisher = Arc::new(move |message| {
        transcript_tx.send(message).unwrap();
    });
    let (service, mut connection, call_id, session_id, manager) =
        establish_call_with(main_agent, transcript_publisher).await;
    let delta =
        |event_id: &str, role: Role, text: &str, end_ms| ProviderConnectionEvent::TranscriptDelta {
            event_id: event_id.into(),
            role,
            text: text.into(),
            start_ms: 0,
            end_ms,
        };

    connection
        .send_event(delta("idle", Role::User, "start work", 10))
        .unwrap();
    assert_eq!(
        transcript_rx.recv().await.unwrap().as_concat_text(),
        "start work"
    );
    connection
        .send_event(ProviderConnectionEvent::DelegationRequested {
            event_id: "start-event".into(),
            delegation_id: "start".into(),
            offset_ms: 10,
        })
        .unwrap();
    let input = starts.recv().await.unwrap();
    assert_eq!(
        input,
        format!("Live conversation context:\nUser: start work\n{DELEGATION_INSTRUCTION}")
    );

    connection
        .send_event(delta("working", Role::Assistant, "working", 20))
        .unwrap();
    connection
        .send_event(delta("detail", Role::User, "also run tests", 30))
        .unwrap();
    for expected in ["working", "also run tests"] {
        assert_eq!(
            transcript_rx.recv().await.unwrap().as_concat_text(),
            expected
        );
    }
    connection
        .send_event(ProviderConnectionEvent::DelegationRequested {
            event_id: "steer-event".into(),
            delegation_id: "steer".into(),
            offset_ms: 30,
        })
        .unwrap();
    let input = steers.recv().await.unwrap();
    assert!(input.contains("GPT-Live: working\nUser: also run tests\n"));
    assert!(!input.contains("start work"));
    assert_eq!(
        connection.next_delegation_update().await.unwrap().text,
        "steered"
    );

    let stored = manager.get_session(&session_id, true).await.unwrap();
    let stored_messages = stored.conversation.unwrap();
    assert_eq!(stored_messages.messages().len(), 3);
    assert_eq!(stored_messages.messages()[2].as_concat_text(), "working");
    assert!(stored_messages.messages()[2].is_user_visible());
    assert!(!stored_messages.messages()[2].is_agent_visible());

    connection
        .send_event(delta("question", Role::Assistant, "Anything else?", 40))
        .unwrap();
    connection
        .send_event(delta("tail", Role::User, "Document it", 50))
        .unwrap();
    for expected in ["Anything else?", "Document it"] {
        assert_eq!(
            transcript_rx.recv().await.unwrap().as_concat_text(),
            expected
        );
    }
    let stop = service.stop_call(&session_id, &call_id);
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
    };
    let (stop, ()) = tokio::join!(stop, provider);
    assert!(stop.is_ok());
    assert_eq!(
        service.availability(Some(&session_id), GooseMode::Auto),
        Err("Live voice is unavailable while this session is busy")
    );
    assert_eq!(
        manager
            .get_session(&session_id, true)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages()
            .len(),
        5
    );

    finish_run.send("done".into()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while service
            .availability(Some(&session_id), GooseMode::Auto)
            .is_err()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let stored = manager
        .get_session(&session_id, true)
        .await
        .unwrap()
        .conversation
        .unwrap();
    let messages = stored.messages();
    assert_eq!(messages.len(), 7);
    assert_eq!(messages[1].as_concat_text(), "start work");
    assert!(!messages[1].is_agent_visible());
    for (message, text) in
        messages[2..6]
            .iter()
            .zip(["working", "also run tests", "Anything else?", "Document it"])
    {
        assert_eq!(message.as_concat_text(), text);
        assert!(message.is_user_visible());
        assert!(!message.is_agent_visible());
    }
    assert!(!messages[6].is_user_visible());
    assert!(messages[6].is_agent_visible());
    assert!(messages[6]
        .as_concat_text()
        .contains("GPT-Live: Anything else?\nUser: Document it"));
    assert!(!messages[6].as_concat_text().contains("also run tests"));
    assert!(!messages[6]
        .as_concat_text()
        .contains(DELEGATION_INSTRUCTION));
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
            service.availability(Some(&session_id), GooseMode::Auto),
            Ok(())
        );
    }
}

#[tokio::test]
async fn provider_terminal_publishes_completion_after_release() {
    let (provider, mut starts) = provider_channel();
    let service = Arc::new(LiveVoiceService::for_test(
        provider,
        Arc::new(ActiveRunRegistry::default()),
    ));
    let start_service = service.clone();
    let (manager, session_id) = live_session([]).await;
    let start_session_id = session_id.clone();
    let start_task = tokio::spawn(async move {
        start_service
            .start_call(
                &start_session_id,
                WebRtcOffer::new("offer".into()).unwrap(),
                manager,
                ignore_transcript_publisher(),
                ignore_main_agent(),
            )
            .await
    });
    let connection = starts
        .recv()
        .await
        .unwrap()
        .accept(WebRtcAnswer::new("answer".into()).unwrap())
        .unwrap();
    let started = start_task.await.unwrap().unwrap();

    connection
        .send_event(ProviderConnectionEvent::Closed)
        .unwrap();
    let completion = wait_for_completion(started.completion_rx).await.unwrap();

    assert_eq!(completion, LiveVoiceCallCompletion::Failed);
    assert_eq!(
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
    );
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
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
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
        service.availability(Some(&session_id), GooseMode::Auto),
        Ok(())
    );
}

#[test]
fn stale_cleanup_cannot_remove_a_later_call() {
    let active_runs = Arc::new(ActiveRunRegistry::default());
    let _call_guard = LiveCallGuard::start(active_runs.clone(), "main-session").unwrap();
    let calls = Arc::new(Mutex::new(HashMap::new()));
    let current_id = LiveVoiceCallId("current".into());
    let (_, completion_rx) = watch::channel(None);
    calls.lock().unwrap().insert(
        "main-session".into(),
        LiveCallControl {
            call_id: current_id.clone(),
            stop_requested: CancellationToken::new(),
            completion_rx,
        },
    );

    remove_call_if_current(&calls, "main-session", &LiveVoiceCallId("stale".into()));

    assert!(matches!(
        calls.lock().unwrap().get("main-session"),
        Some(control) if control.call_id == current_id
    ));
    assert!(active_runs.is_active("main-session"));
}
