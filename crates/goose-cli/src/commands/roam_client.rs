//! Thin ACP client UI onto a remote roaming agent.
//!
//! Per design doc §9: `roam connect` is NOT a provider wrapper for a local
//! agent loop. The **host** runs the real agent (its tools, working dir,
//! shell); this side is just an ACP *client* that opens a session, sends
//! prompts, and renders `session/update` notifications to the terminal.
//!
//! We deliberately advertise no client filesystem/terminal capabilities and do
//! not send our local cwd — the host imposes the `share` working directory.

use std::io::Write;

use tokio::io::{AsyncBufReadExt, BufReader};

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, ListSessionsRequest, LoadSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionInfo, SessionNotification, SessionUpdate,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Client, ConnectionTo};
use anyhow::Result;
use goose_roaming::RoamingClientStream;

/// Run an interactive ACP session over an authorized roaming stream, reading
/// prompts from stdin until EOF / quit.
pub async fn run_interactive(stream: RoamingClientStream, agent_label: String) -> Result<()> {
    let (send, recv, conn) = stream.into_futures_io();
    let transport = agent_client_protocol::ByteStreams::new(send, recv);

    Client
        .builder()
        .name("goose-roam")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                render_update(&notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                // The host runs the agent, so tool-permission prompts originate
                // there. Present them to the local user and forward the choice.
                let outcome = prompt_permission(&request);
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            let init = cx
                .send_request(InitializeRequest::new(ProtocolVersion::LATEST))
                .block_task()
                .await?;
            eprintln!(
                "connected to remote agent `{agent_label}` (protocol {:?})",
                init.protocol_version
            );
            eprintln!("type a message and press enter; Ctrl-D or /quit to end.\n");

            // The host imposes its own working directory and ignores whatever we
            // send here (our local path is meaningless on the host machine). ACP
            // still requires a syntactically-absolute cwd, so send a placeholder.
            let cwd = std::path::PathBuf::from("/");
            let result = cx
                .build_session(cwd)
                .block_task()
                .run_until(async |mut session| {
                    tracing::debug!(session_id = ?session.session_id(), "roam client: session created");
                    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
                    loop {
                        eprint!("› ");
                        let _ = std::io::stderr().flush();
                        let line = match stdin.next_line().await {
                            Ok(Some(l)) => l,
                            Ok(None) | Err(_) => break, // EOF
                        };
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        if line == "/quit" || line == "/exit" {
                            break;
                        }
                        session.send_prompt(line)?;
                        // Drain updates until the turn completes; chunks are
                        // rendered live via the notification handler above.
                        let _ = session.read_to_string().await?;
                        println!();
                    }
                    Ok(())
                })
                .await;
            if let Err(e) = &result {
                tracing::warn!("roam client: session ended with error: {e:?}");
            }
            result
        })
        .await?;

    drop(conn);
    Ok(())
}

/// One-shot delegation: open a remote session, send a single task, return the
/// agent's final text response. No interactive loop, no local stdin.
///
/// This is the reusable core a future `roam__delegate` model tool will call.
/// Permission requests are auto-cancelled: a delegated (agent-driven) session
/// must not block waiting for a human, and the caller isn't a person who can
/// answer. Loop/cost safety is the caller's concern (bounded turns/deadline).
pub async fn delegate(
    stream: RoamingClientStream,
    task: String,
    session: Option<String>,
) -> Result<String> {
    let (send, recv, conn) = stream.into_futures_io();
    let transport = agent_client_protocol::ByteStreams::new(send, recv);
    let collected = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = collected.clone();
    let replay_sink = collected.clone();

    Client
        .builder()
        .name("goose-roam-delegate")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                    if let ContentBlock::Text(text) = &chunk.content {
                        sink.lock().unwrap().push_str(&text.text);
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |_request: RequestPermissionRequest, responder, _cx| {
                // Agent-driven session: never wait on a human. Auto-cancel.
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::LATEST))
                .block_task()
                .await?;
            // The host imposes its own working directory and ignores whatever we
            // send; ACP still requires a syntactically-absolute cwd.
            let cwd = std::path::PathBuf::from("/");
            match session {
                // Resume an existing remote session. The final text is
                // collected by the notification handler above, so a raw
                // session/prompt awaited to completion is all we need — no
                // ActiveSession attachment required.
                Some(id) => {
                    let session_id = SessionId::from(id);
                    cx.send_request(LoadSessionRequest::new(session_id.clone(), cwd))
                        .block_task()
                        .await?;
                    // Loading replays the session's history as message chunks,
                    // which the notification handler collected. Discard them so
                    // the result is only the new response to this task.
                    replay_sink.lock().unwrap().clear();
                    cx.send_request(PromptRequest::new(session_id, vec![task.clone().into()]))
                        .block_task()
                        .await?;
                    Ok(())
                }
                None => {
                    cx.build_session(cwd)
                        .block_task()
                        .run_until(async |mut session| {
                            session.send_prompt(&task)?;
                            let _ = session.read_to_string().await?;
                            Ok(())
                        })
                        .await
                }
            }
        })
        .await?;

    drop(conn);
    let result = collected.lock().unwrap().clone();
    Ok(result)
}

/// List the remote agent's sessions via ACP `session/list`. Roaming adds no
/// session semantics — this is plain ACP over the authorized stream.
pub async fn list_sessions(stream: RoamingClientStream) -> Result<Vec<SessionInfo>> {
    let (send, recv, conn) = stream.into_futures_io();
    let transport = agent_client_protocol::ByteStreams::new(send, recv);

    let sessions = Client
        .builder()
        .name("goose-roam-list")
        .on_receive_notification(
            async move |_notification: SessionNotification, _cx| Ok(()),
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::LATEST))
                .block_task()
                .await?;
            let response = cx
                .send_request(ListSessionsRequest::default())
                .block_task()
                .await?;
            Ok(response.sessions)
        })
        .await?;

    drop(conn);
    Ok(sessions)
}

fn render_update(update: &SessionUpdate) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = &chunk.content {
                print!("{}", text.text);
                let _ = std::io::stdout().flush();
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            if let ContentBlock::Text(text) = &chunk.content {
                eprint!("\x1b[2m{}\x1b[0m", text.text);
            }
        }
        SessionUpdate::ToolCall(tool_call) => {
            eprintln!("\n🔧 {}", tool_call.title);
        }
        SessionUpdate::ToolCallUpdate(update) => {
            if let Some(status) = &update.fields.status {
                eprintln!("   [{status:?}]");
            }
        }
        _ => {}
    }
}

fn prompt_permission(request: &RequestPermissionRequest) -> RequestPermissionOutcome {
    eprintln!("\n⚠️  the remote agent requests permission:");
    for (i, opt) in request.options.iter().enumerate() {
        eprintln!("   {}) {}", i + 1, opt.name);
    }
    eprint!("choose a number (anything else cancels): ");
    let _ = std::io::stderr().flush();

    // Fail closed: option 1 is allow-always for goose hosts, so EOF, an empty
    // line, or a typo must cancel rather than silently granting permission.
    let Some(choice) = read_line().and_then(|l| l.trim().parse::<usize>().ok()) else {
        eprintln!("   cancelled");
        return RequestPermissionOutcome::Cancelled;
    };

    match choice
        .checked_sub(1)
        .and_then(|idx| request.options.get(idx))
    {
        Some(opt) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
            opt.option_id.clone(),
        )),
        None => {
            eprintln!("   cancelled");
            RequestPermissionOutcome::Cancelled
        }
    }
}

fn read_line() -> Option<String> {
    eprint!("› ");
    let _ = std::io::stderr().flush();
    let mut buf = String::new();
    match std::io::stdin().read_line(&mut buf) {
        Ok(0) => None, // EOF
        Ok(_) => Some(buf),
        Err(_) => None,
    }
}
