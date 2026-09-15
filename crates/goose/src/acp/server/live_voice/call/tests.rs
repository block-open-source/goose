use super::*;
use async_trait::async_trait;
use tokio::sync::oneshot;

struct TestConnection {
    stopped: Option<oneshot::Sender<()>>,
}

#[async_trait]
impl ProviderConnection for TestConnection {
    async fn next_event(&mut self) -> ProviderConnectionEvent {
        std::future::pending().await
    }

    async fn send_delegation_update(
        &mut self,
        _update: goose_providers::live_voice_provider::DelegationUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(stopped) = self.stopped.take() {
            let _ = stopped.send(());
        }
        Ok(())
    }
}

#[tokio::test]
async fn a_call_owns_and_stops_its_provider_connection() {
    let (stopped, did_stop) = oneshot::channel();
    let mut call = LiveVoiceCall::new(
        "test-session".into(),
        LiveVoiceCallId("live-test".into()),
        Box::new(TestConnection {
            stopped: Some(stopped),
        }),
    );

    call.cleanup_provider().await.unwrap();
    did_stop.await.unwrap();
}

#[test]
fn delegation_preparation_respects_offset_and_suppresses_duplicates() {
    let mut call = LiveVoiceCall::new(
        "test-session".into(),
        LiveVoiceCallId("live-test".into()),
        Box::new(TestConnection { stopped: None }),
    );
    call.record_transcript("1".into(), Role::Assistant, "ready", 5);
    call.record_transcript("2".into(), Role::User, "do ", 10);
    call.record_transcript("3".into(), Role::User, "this", 20);
    call.record_transcript("4".into(), Role::Assistant, "crossing", 25);
    call.record_transcript("5".into(), Role::Assistant, "later", 40);

    let DelegationDecision::Accept(input) =
        call.handle_delegation_request("event-1".into(), "delegation-1".into(), 20)
    else {
        panic!("delegation should be accepted");
    };
    assert_eq!(
        input,
        "Live conversation context:\nGPT-Live: ready\nUser: do this"
    );
    call.transcript.mark_context_sent_to_main_agent_through(20);
    assert!(matches!(
        call.handle_delegation_request("event-1".into(), "delegation-1".into(), 20),
        DelegationDecision::Ignore
    ));

    call.record_transcript("6".into(), Role::User, " late detail", 20)
        .unwrap();
    assert_eq!(
        call.transcript
            .raw_transcript_entries_waiting_to_save()
            .last()
            .unwrap()
            .as_concat_text(),
        "crossinglater"
    );
    let DelegationDecision::Accept(continuation) =
        call.handle_delegation_request("event-2".into(), "delegation-2".into(), 20)
    else {
        panic!("continuation should be accepted");
    };
    assert_eq!(
        continuation,
        "Live conversation context:\nUser: late detail"
    );
    call.transcript.mark_context_sent_to_main_agent_through(20);

    assert!(matches!(
        call.handle_delegation_request("event-3".into(), "delegation-3".into(), 19),
        DelegationDecision::Reject(_)
    ));

    call.record_transcript("7".into(), Role::User, "again", 50);
    let DelegationDecision::Accept(next) =
        call.handle_delegation_request("event-4".into(), "delegation-4".into(), 50)
    else {
        panic!("later continuation should be accepted");
    };
    assert_eq!(
        next,
        "Live conversation context:\nGPT-Live: crossinglater\nUser: again"
    );
    call.transcript.mark_context_sent_to_main_agent_through(50);

    let mut missing_user = LiveVoiceCall::new(
        "test-session".into(),
        LiveVoiceCallId("live-test-2".into()),
        Box::new(TestConnection { stopped: None }),
    );
    missing_user.record_transcript("1".into(), Role::Assistant, "hello", 10);
    assert!(matches!(
        missing_user.handle_delegation_request("event-1".into(), "delegation-1".into(), 10),
        DelegationDecision::Reject(_)
    ));
}

#[test]
fn context_waiting_for_main_agent_excludes_instruction_and_clears_after_handoff() {
    let mut call = LiveVoiceCall::new(
        "test-session".into(),
        LiveVoiceCallId("live-test".into()),
        Box::new(TestConnection { stopped: None }),
    );
    call.record_transcript("1".into(), Role::Assistant, "Anything else?", 10);
    call.record_transcript("2".into(), Role::User, "No thanks", 20);

    let context = call.transcript.context_waiting_for_main_agent().unwrap();
    assert_eq!(
        context,
        "Live conversation context:\nGPT-Live: Anything else?\nUser: No thanks"
    );
    assert_eq!(
        call.transcript.context_waiting_for_main_agent().unwrap(),
        context
    );

    call.record_transcript("3".into(), Role::User, "Already shared", 30);
    call.transcript.mark_context_sent_to_main_agent_through(30);
    assert!(call.transcript.context_waiting_for_main_agent().is_none());
}

#[test]
fn transcript_grouping_projects_deltas_and_finalizes_messages() {
    let mut call = LiveVoiceCall::new(
        "test-session".into(),
        LiveVoiceCallId("live-test".into()),
        Box::new(TestConnection { stopped: None }),
    );
    let first = call
        .record_transcript("1".into(), Role::User, "hello", 10)
        .unwrap();
    assert!(first.is_user_visible());
    assert!(!first.is_agent_visible());
    let message_id = first.id.clone();
    let second = call
        .record_transcript("2".into(), Role::User, " world", 20)
        .unwrap();
    assert_eq!(second.id, message_id);
    assert_eq!(second.as_concat_text(), " world");

    assert!(call
        .record_transcript("2".into(), Role::User, " world", 20)
        .is_none());

    let role_change = call
        .record_transcript("3".into(), Role::Assistant, "hello", 30)
        .unwrap();
    assert_eq!(
        call.transcript
            .raw_transcript_entries_waiting_to_save()
            .last()
            .unwrap()
            .as_concat_text(),
        "hello world"
    );
    assert_eq!(role_change.role, Role::Assistant);
    assert!(role_change.is_user_visible());
    assert!(!role_change.is_agent_visible());
}
