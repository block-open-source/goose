use super::*;
use goose_providers::live_voice_provider::{
    fake::{channel, FakeConnectionDriver, FakeStartRequest},
    LiveVoiceCapabilities, ProviderCommand, ProviderDelegation, ProviderDelegationId,
    ProviderDelegationTarget, ProviderDispatchResult, ProviderEvent, ProviderEventStreamError,
    ProviderStartupObservation,
};
use std::time::Duration;
use tokio::{sync::mpsc, time::timeout};

fn coordinator() -> (
    LiveVoiceCoordinator,
    mpsc::UnboundedReceiver<FakeStartRequest>,
) {
    let (provider, starts) = channel(LiveVoiceCapabilities::default());
    let short = Duration::from_millis(60);
    (
        LiveVoiceCoordinator::with_deadlines(
            provider,
            Deadlines {
                provider_start: short,
                readiness: short,
                acknowledgement: short,
                provider_close: Duration::from_millis(20),
                cleanup: short,
            },
        ),
        starts,
    )
}

fn owner() -> LiveOwnerToken {
    LiveOwnerToken("owner".into())
}
fn session() -> LiveSessionId {
    LiveSessionId("live-session".into())
}

fn request(attempt: u64) -> (StartRequest, mpsc::UnboundedReceiver<LiveVoiceUpdate>) {
    let (updates, receiver) = mpsc::unbounded_channel();
    (
        StartRequest {
            owner: owner(),
            live_session_id: session(),
            linked_work_session_id: Some(WorkSessionId("busy-work-session".into())),
            attempt,
            config: Default::default(),
            media: goose_providers::live_voice_provider::LiveVoiceMediaRequest::WebRtc {
                offer_sdp: "offer".into(),
            },
            updates,
        },
        receiver,
    )
}

fn accept(start: FakeStartRequest) -> FakeConnectionDriver {
    start
        .accept(
            goose_providers::live_voice_provider::LiveVoiceMediaAnswer::WebRtc {
                answer_sdp: "answer".into(),
            },
        )
        .unwrap()
}

async fn connected() -> (
    LiveVoiceCoordinator,
    StartResponse,
    FakeConnectionDriver,
    mpsc::UnboundedReceiver<LiveVoiceUpdate>,
) {
    let (coordinator, mut starts) = coordinator();
    let (request, updates) = request(1);
    let task = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.start(request).await.unwrap() })
    };
    let driver = accept(starts.recv().await.unwrap());
    (coordinator, task.await.unwrap(), driver, updates)
}

fn command(coordinator: &LiveVoiceCoordinator, started: &StartResponse, command: LiveVoiceCommand) {
    coordinator
        .command(&owner(), &session(), &started.live_connection_id, command)
        .unwrap();
}

fn ready_media(coordinator: &LiveVoiceCoordinator, started: &StartResponse) {
    for observation in [
        MediaReadinessObservation::AnswerApplied,
        MediaReadinessObservation::RemoteTrackLive,
        MediaReadinessObservation::PeerConnected,
        MediaReadinessObservation::DataChannelOpen,
    ] {
        command(
            coordinator,
            started,
            LiveVoiceCommand::ObserveMedia(observation),
        );
    }
}

async fn update_matching(
    updates: &mut mpsc::UnboundedReceiver<LiveVoiceUpdate>,
    predicate: impl Fn(&LiveVoiceEvent) -> bool,
) -> LiveVoiceEvent {
    loop {
        let update = timeout(Duration::from_millis(200), updates.recv())
            .await
            .unwrap()
            .unwrap();
        if predicate(&update.event) {
            return update.event;
        }
    }
}

#[tokio::test]
async fn attempt_registration_is_atomic_and_cancellation_is_remembered() {
    let (coordinator, mut starts) = coordinator();
    coordinator
        .stop(&owner(), &session(), StopTarget::Attempt(1))
        .unwrap();
    let (stale, _) = request(1);
    assert_eq!(
        coordinator.start(stale).await.unwrap_err(),
        CoordinatorError::StaleAttempt
    );

    let (first, _) = request(2);
    let task = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.start(first).await })
    };
    let pending = starts.recv().await.unwrap();
    let (duplicate, _) = request(3);
    assert_eq!(
        coordinator.start(duplicate).await.unwrap_err(),
        CoordinatorError::Busy
    );
    coordinator
        .stop(&owner(), &session(), StopTarget::Attempt(2))
        .unwrap();
    pending.reject("cancelled").unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        CoordinatorError::Cancelled
    );
}

#[tokio::test]
async fn activation_requires_provider_and_all_media_readiness() {
    let (coordinator, started, driver, mut updates) = connected().await;
    driver.emit(ProviderEvent::Ready).unwrap();
    assert!(timeout(
        Duration::from_millis(10),
        update_matching(&mut updates, |kind| matches!(
            kind,
            LiveVoiceEvent::State {
                connection: LiveConnectionState::Active,
                ..
            }
        ))
    )
    .await
    .is_err());
    ready_media(&coordinator, &started);
    update_matching(&mut updates, |kind| {
        matches!(
            kind,
            LiveVoiceEvent::State {
                connection: LiveConnectionState::Active,
                provider_input_enabled: false,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn bootstrap_readiness_is_validated_by_the_provider() {
    let (coordinator, started, mut driver, mut updates) = connected().await;
    ready_media(&coordinator, &started);
    command(
        &coordinator,
        &started,
        LiveVoiceCommand::ObserveBootstrap(ProviderStartupObservation("provider-session".into())),
    );
    assert!(matches!(
        driver.next_command().await.unwrap(),
        ProviderCommand::ValidateStartupObservation { observation, .. }
            if observation == ProviderStartupObservation("provider-session".into())
    ));
    driver.emit(ProviderEvent::Ready).unwrap();
    update_matching(&mut updates, |event| {
        matches!(
            event,
            LiveVoiceEvent::State {
                connection: LiveConnectionState::Active,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn rejected_bootstrap_identity_fails_the_connection() {
    let (coordinator, started, driver, mut updates) = connected().await;
    driver.set_dispatch_result(ProviderDispatchResult::Rejected("identity mismatch".into()));
    command(
        &coordinator,
        &started,
        LiveVoiceCommand::ObserveBootstrap(ProviderStartupObservation("wrong-session".into())),
    );
    update_matching(&mut updates, |event| {
        matches!(
            event,
            LiveVoiceEvent::Terminal {
                reason: LiveTerminalReason::CommandFailed(error),
                ..
            } if error == "identity mismatch"
        )
    })
    .await;
    assert!(!coordinator.has_connection(&owner(), &session()));
}

#[tokio::test]
async fn readiness_timeout_is_terminal_and_releases_the_generation() {
    let (coordinator, started, _driver, mut updates) = connected().await;
    ready_media(&coordinator, &started);
    update_matching(&mut updates, |kind| {
        matches!(
            kind,
            LiveVoiceEvent::Terminal {
                reason: LiveTerminalReason::ReadinessTimedOut,
                ..
            }
        )
    })
    .await;
    assert!(!coordinator.has_connection(&owner(), &session()));
}

#[tokio::test]
async fn stale_resume_ack_cannot_enable_input_after_mute() {
    let (coordinator, started, mut driver, mut updates) = connected().await;
    driver.emit(ProviderEvent::Ready).unwrap();
    ready_media(&coordinator, &started);
    let resume = match driver.next_command().await.unwrap() {
        ProviderCommand::ResumeInput { command_id } => command_id,
        other => panic!("unexpected {other:?}"),
    };
    command(&coordinator, &started, LiveVoiceCommand::SetMuted(true));
    command(&coordinator, &started, LiveVoiceCommand::SetMuted(false));
    command(&coordinator, &started, LiveVoiceCommand::SetMuted(true));
    driver
        .emit(ProviderEvent::InputResumed { command_id: resume })
        .unwrap();
    let pause = match driver.next_command().await.unwrap() {
        ProviderCommand::PauseInput { command_id } => command_id,
        other => panic!("unexpected {other:?}"),
    };
    while let Ok(Some(update)) = timeout(Duration::from_millis(10), updates.recv()).await {
        let event = update.event;
        assert!(!matches!(
            event,
            LiveVoiceEvent::State {
                muted_intent: true,
                provider_input_enabled: true,
                ..
            }
        ));
    }
    driver
        .emit(ProviderEvent::InputPaused { command_id: pause })
        .unwrap();
    update_matching(&mut updates, |event| {
        matches!(
            event,
            LiveVoiceEvent::State {
                muted_intent: true,
                provider_input_enabled: false,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn delegations_are_deduplicated_and_keep_the_provider_identity() {
    let (_coordinator, _started, mut driver, mut updates) = connected().await;
    let item = ProviderDelegation {
        id: ProviderDelegationId("d1".into()),
        target: ProviderDelegationTarget::Client,
        task: "work".into(),
        source_turn_id: None,
    };
    driver
        .emit(ProviderEvent::DelegationRequested(item.clone()))
        .unwrap();
    driver
        .emit(ProviderEvent::DelegationRequested(item))
        .unwrap();
    let (command_id, delegation_id) = match driver.next_command().await.unwrap() {
        ProviderCommand::DeliverDelegationResult {
            command_id,
            delegation_id,
            result,
        } => {
            assert!(result.contains("can't start Goose work"));
            (command_id, delegation_id)
        }
        other => panic!("unexpected {other:?}"),
    };
    assert!(timeout(Duration::from_millis(10), driver.next_command())
        .await
        .is_err());
    driver
        .emit(ProviderEvent::DelegationResultAccepted {
            command_id,
            delegation_id: delegation_id.clone(),
        })
        .unwrap();
    update_matching(
        &mut updates,
        |event| matches!(event, LiveVoiceEvent::DelegationDelivered(id) if id == &delegation_id),
    )
    .await;
}

#[tokio::test]
async fn dropped_start_request_cleans_up_a_late_connection() {
    let (coordinator, mut starts) = coordinator();
    let (request, _) = request(1);
    let task = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.start(request).await })
    };
    let pending = starts.recv().await.unwrap();
    task.abort();
    let mut driver = accept(pending);
    driver.next_close().await.unwrap().send(Ok(())).unwrap();
    tokio::time::sleep(Duration::from_millis(70)).await;
    assert!(!coordinator.has_connection(&owner(), &session()));
}

#[tokio::test]
async fn receiver_lag_releases_state_even_if_updates_are_dropped() {
    let (coordinator, _started, driver, updates) = connected().await;
    drop(updates);
    driver.fail(ProviderEventStreamError::Lagged(2)).unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!coordinator.has_connection(&owner(), &session()));
}

#[tokio::test]
async fn fallback_cleanup_does_not_claim_remote_acknowledgement() {
    let (coordinator, started, mut driver, mut updates) = connected().await;
    driver
        .fail(ProviderEventStreamError::Failed("sideband lost".into()))
        .unwrap();
    driver.next_close().await.unwrap().send(Ok(())).unwrap();
    update_matching(&mut updates, |event| {
        matches!(event, LiveVoiceEvent::RequestMediaCleanup(_))
    })
    .await;
    command(
        &coordinator,
        &started,
        LiveVoiceCommand::MediaCleanupResult(MediaCleanupResult::Dispatched),
    );
    let terminal = update_matching(&mut updates, |event| {
        matches!(event, LiveVoiceEvent::Terminal { .. })
    })
    .await;
    assert!(matches!(
        terminal,
        LiveVoiceEvent::Terminal {
            remote_acknowledged: false,
            provider_close_completed: true,
            media_cleanup: Some(MediaCleanupResult::Dispatched),
            ..
        }
    ));
}
