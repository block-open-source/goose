//! End-to-end tests for the roaming ACP *client* helpers over the real iroh
//! transport.
//!
//! Roaming is just an authenticated p2p ACP transport, so a stub ACP *agent*
//! stands in for goose's real `serve` and implements the session surface the
//! client exercises: `session/list`, `session/new`, `session/load`,
//! `session/prompt`. This proves `roam_client::list_sessions` and the
//! session-aware `roam_client::delegate` drive plain ACP correctly across a
//! roaming stream — no LLM/provider required.

#![cfg(feature = "roaming")]

use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, SessionId, SessionInfo, SessionNotification,
    SessionUpdate, StopReason,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent as SacpAgent, Client, ConnectionTo};
use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use futures::io::{AsyncRead, AsyncWrite};

use goose_cli::commands::roam_client;
use goose_roaming::{
    AcpStreamServer, Directory, EndpointId, RelaySettings, RoamingConfig, RoamingIdentity,
    RoamingNode, TrustBook,
};

/// A stub ACP agent serving the session surface the client uses. It reports one
/// fixed session in `session/list`, echoes the prompt back prefixed so tests can
/// assert what was sent, and tags the reply with whether the session was created
/// (`session/new`) or loaded (`session/load`).
struct StubAcpAgent;

impl AcpStreamServer for StubAcpAgent {
    fn serve_stream(
        &self,
        _client: EndpointId,
        recv: Box<dyn AsyncRead + Send + Unpin>,
        send: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let transport = agent_client_protocol::ByteStreams::new(send, recv);

            SacpAgent
                .builder()
                .name("stub-acp-agent")
                .on_receive_request(
                    async move |_req: InitializeRequest, responder, _cx| {
                        responder.respond(InitializeResponse::new(ProtocolVersion::LATEST))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: ListSessionsRequest, responder, _cx| {
                        let info = SessionInfo::new(SessionId::from("sess-42"), "/work")
                            .title("Existing session");
                        responder.respond(ListSessionsResponse::new(vec![info]))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: NewSessionRequest, responder, _cx| {
                        responder.respond(NewSessionResponse::new(SessionId::from("sess-new")))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: LoadSessionRequest, responder, _cx| {
                        responder.respond(LoadSessionResponse::default())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: PromptRequest, responder, cx| {
                        // Reflect the loaded/new session id + the prompt text so
                        // the client's collected output is assertable.
                        let text = req
                            .prompt
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text(t) => Some(t.text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        let reply = format!("session={} prompt={text}", req.session_id);
                        cx.send_notification(SessionNotification::new(
                            req.session_id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::from(reply),
                            )),
                        ))?;
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |_cx: ConnectionTo<Client>| {
                    // Keep the connection alive until the client hangs up.
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await
                .map_err(|e| anyhow!(e))?;
            Ok(())
        })
    }

    fn agent_id(&self) -> String {
        "stub-acp-agent".to_string()
    }
}

fn loopback() -> std::net::SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

async fn bind_node(trust: TrustBook) -> Arc<RoamingNode> {
    RoamingNode::bind(RoamingConfig {
        identity: RoamingIdentity::generate(),
        relay: RelaySettings::Disabled,
        trust,
        trust_path: None,
        directory: Directory::new(),
        bind_addr: Some(loopback()),
        relay_tls: None,
    })
    .await
    .expect("bind node")
}

/// Bind a host serving the stub ACP agent and return a client stream connected
/// to it over the real (relay-disabled, loopback) iroh transport. The host
/// accepts the client's key first (mutual, key-based trust).
async fn connect_to_stub() -> (Arc<RoamingNode>, goose_roaming::RoamingClientStream) {
    let host = bind_node(TrustBook::new()).await;
    host.share(Arc::new(StubAcpAgent)).await.expect("share");

    let client = bind_node(TrustBook::new()).await;
    host.trust().lock().await.accept(&client.endpoint_id());

    let stream = client
        .connect_with_addr(host.endpoint().addr(), Some("test".into()))
        .await
        .expect("client connects");
    (client, stream)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_over_roaming() {
    let (client, stream) = connect_to_stub().await;
    let sessions = roam_client::list_sessions(stream).await.expect("list");
    client.shutdown().await.ok();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, SessionId::from("sess-42"));
    assert_eq!(sessions[0].title.as_deref(), Some("Existing session"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegate_new_session_over_roaming() {
    let (client, stream) = connect_to_stub().await;
    let out = roam_client::delegate(stream, "do the thing".into(), None)
        .await
        .expect("delegate");
    client.shutdown().await.ok();

    // A fresh session (session/new) was used, and our prompt reached the agent.
    assert!(out.contains("session=sess-new"), "got: {out}");
    assert!(out.contains("prompt=do the thing"), "got: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegate_loads_named_session_over_roaming() {
    let (client, stream) = connect_to_stub().await;
    let out = roam_client::delegate(stream, "continue".into(), Some("sess-42".into()))
        .await
        .expect("delegate");
    client.shutdown().await.ok();

    // The named session was loaded (session/load) and driven, not a fresh one.
    assert!(out.contains("session=sess-42"), "got: {out}");
    assert!(out.contains("prompt=continue"), "got: {out}");
}
