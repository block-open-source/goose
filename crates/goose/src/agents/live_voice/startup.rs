use super::{
    cleanup::cleanup,
    lifecycle::{ConnectionKey, ConnectionLifecycle, LifecycleAction},
    release,
    types::*,
    update_sink::UpdateSink,
    CoordinatorInner,
};
use goose_providers::live_voice_provider::ProviderConnection;
use std::sync::Arc;
use tokio::{
    sync::{mpsc, oneshot},
    time::{timeout_at, Instant},
};

pub(super) struct ConnectionContext {
    pub inner: Arc<CoordinatorInner>,
    pub key: ConnectionKey,
    pub id: LiveConnectionId,
    pub request: StartRequest,
    pub actions: mpsc::UnboundedReceiver<LifecycleAction>,
    pub reply: Option<oneshot::Sender<Result<StartResponse, CoordinatorError>>>,
}

pub(super) async fn run(mut args: ConnectionContext) {
    let updates = UpdateSink::new(&args.request, &args.id);
    updates.state(LiveConnectionState::Connecting, false, false, None);

    let provider = args.inner.provider.clone();
    let config = args.request.config.clone();
    let media = args.request.media.clone();
    let mut start = tokio::spawn(async move { provider.start(config, media).await });
    let start_deadline = Instant::now() + args.inner.deadlines.provider_start;

    let connection = tokio::select! {
        action = args.actions.recv() => {
            let reason = action.map_or(LiveTerminalReason::Cancelled, stop_reason);
            reply_cancelled(&mut args);
            dispose_late_start(args, updates, start, start_deadline, reason).await;
            return;
        }
        result = timeout_at(start_deadline, &mut start) => match result {
            Ok(Ok(Ok(connection))) => connection,
            Ok(Ok(Err(error))) => return start_failed(args, updates, error.to_string()),
            Ok(Err(error)) => return start_failed(args, updates, format!("provider setup task failed: {error}")),
            Err(_) => {
                start.abort();
                return start_failed(args, updates, "provider setup timed out".into());
            }
        }
    };

    if let Ok(action) = args.actions.try_recv() {
        let reason = stop_reason(action);
        reply_cancelled(&mut args);
        cleanup(&mut args, &updates, connection, reason, false, None, false).await;
        return;
    }

    let response = StartResponse {
        live_session_id: args.request.live_session_id.clone(),
        linked_work_session_id: args.request.linked_work_session_id.clone(),
        attempt: args.request.attempt,
        live_connection_id: args.id.clone(),
        media_answer: connection.media_answer.clone(),
    };
    if args
        .reply
        .take()
        .expect("start reply available")
        .send(Ok(response))
        .is_err()
    {
        cleanup(
            &mut args,
            &updates,
            connection,
            LiveTerminalReason::Cancelled,
            false,
            None,
            false,
        )
        .await;
        return;
    }

    ConnectionLifecycle::new(&mut args, &updates, connection)
        .run()
        .await;
}

fn stop_reason(action: LifecycleAction) -> LiveTerminalReason {
    match action {
        LifecycleAction::Stop(reason) => reason,
        LifecycleAction::Command(LiveVoiceCommand::MediaFailed(error)) => {
            LiveTerminalReason::MediaFailed(error)
        }
        LifecycleAction::Command(_) => LiveTerminalReason::Cancelled,
    }
}

fn reply_cancelled(args: &mut ConnectionContext) {
    let _ = args
        .reply
        .take()
        .expect("start reply available")
        .send(Err(CoordinatorError::Cancelled));
}

async fn dispose_late_start(
    mut args: ConnectionContext,
    updates: UpdateSink,
    mut start: tokio::task::JoinHandle<anyhow::Result<ProviderConnection>>,
    start_deadline: Instant,
    reason: LiveTerminalReason,
) {
    match timeout_at(start_deadline, &mut start).await {
        Ok(Ok(Ok(connection))) => {
            cleanup(&mut args, &updates, connection, reason, false, None, false).await
        }
        _ => {
            start.abort();
            finish_without_connection(&args, &updates, reason);
        }
    }
}

fn start_failed(mut args: ConnectionContext, updates: UpdateSink, message: String) {
    let _ = args
        .reply
        .take()
        .expect("start reply available")
        .send(Err(CoordinatorError::StartFailed(message.clone())));
    finish_without_connection(&args, &updates, LiveTerminalReason::StartFailed(message));
}

fn finish_without_connection(
    args: &ConnectionContext,
    updates: &UpdateSink,
    reason: LiveTerminalReason,
) {
    let failed = matches!(
        reason,
        LiveTerminalReason::StartFailed(_) | LiveTerminalReason::MediaFailed(_)
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
    updates.terminal(reason, false, false, None, None);
    release(&args.inner, &args.key, &args.id);
}
