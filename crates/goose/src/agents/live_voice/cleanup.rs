use super::{
    lifecycle::LifecycleAction, release, startup::ConnectionContext, types::*,
    update_sink::UpdateSink,
};
use goose_providers::live_voice_provider::{ProviderConnection, ProviderEvent};
use tokio::time::{sleep_until, timeout, Instant};

pub(super) async fn cleanup(
    args: &mut ConnectionContext,
    updates: &UpdateSink,
    connection: ProviderConnection,
    reason: LiveTerminalReason,
    remote_acknowledged: bool,
    initial_usage: Option<goose_providers::live_voice_provider::ProviderUsage>,
    mut fallback: bool,
) {
    updates.state(LiveConnectionState::Ending, true, false, None);
    let deadline = Instant::now() + args.inner.deadlines.cleanup;
    let mut events = connection.events;
    let mut close = Box::pin(timeout(
        args.inner.deadlines.provider_close,
        connection.control.close(),
    ));
    let mut close_finished = false;
    let mut close_completed = false;
    let mut remote_closed = remote_acknowledged;
    let mut media_cleanup = None;
    let mut usage = initial_usage;
    let mut events_open = true;
    let mut actions_open = true;

    if fallback {
        updates.send(LiveVoiceEvent::RequestMediaCleanup(
            args.inner.deadlines.cleanup,
        ));
    }
    while !(close_finished && remote_closed) {
        tokio::select! {
            result = &mut close, if !close_finished => {
                close_finished = true;
                close_completed = matches!(result, Ok(Ok(())));
                if !close_completed && !fallback {
                    fallback = true;
                    updates.send(LiveVoiceEvent::RequestMediaCleanup(
                        deadline.saturating_duration_since(Instant::now()),
                    ));
                }
            }
            event = events.recv(), if events_open => match event {
                Some(Ok(ProviderEvent::RemoteClosed { usage: final_usage, .. })) => {
                    remote_closed = true;
                    if final_usage.is_some() {
                        usage = final_usage;
                    }
                }
                Some(Ok(ProviderEvent::UsageUpdated(value))) => usage = Some(value),
                Some(_) => {}
                None => events_open = false,
            },
            action = args.actions.recv(), if actions_open => match action {
                Some(LifecycleAction::Command(LiveVoiceCommand::MediaCleanupResult(value)))
                    if fallback && media_cleanup.is_none() => media_cleanup = Some(value),
                Some(_) => {}
                None => actions_open = false,
            },
            _ = sleep_until(deadline) => break,
        }
    }

    let failed = !matches!(
        reason,
        LiveTerminalReason::Stopped
            | LiveTerminalReason::Cancelled
            | LiveTerminalReason::OwnerLost
            | LiveTerminalReason::SessionClosed
            | LiveTerminalReason::ProviderClosed(_)
    );
    updates.state(
        if failed {
            LiveConnectionState::Failed
        } else {
            LiveConnectionState::Closed
        },
        true,
        false,
        None,
    );
    updates.terminal(reason, remote_closed, close_completed, media_cleanup, usage);
    release(&args.inner, &args.key, &args.id);
}
