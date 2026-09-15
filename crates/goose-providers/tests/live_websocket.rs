#![cfg(feature = "live-websocket")]

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use goose_providers::openai_live::{OpenAiLiveClient, OpenAiLiveSessionConfig};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_hdr_async, tungstenite::Message};

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn websocket_connect_performs_handshake_and_waits_until_ready() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hdr_async(stream, |request: &http::Request<()>, response| {
            assert_eq!(request.headers()["authorization"], "Bearer test-key");
            Ok(response)
        })
        .await
        .unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let Message::Text(text) = message else {
            panic!("expected initial text message");
        };
        let session_start: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(session_start["type"], "session.start");
        socket
            .send(Message::Text(
                json!({
                    "type": "session.started",
                    "event_id": "event_started_1",
                    "session": { "id": "session_1" }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    });

    let connected = OpenAiLiveClient::new("test-key")
        .with_websocket_endpoint(format!("ws://{address}/v1/live/sessions"))
        .websocket(OpenAiLiveSessionConfig {
            model: "gpt-live-test".into(),
            instructions: "test".into(),
            voice: None,
            input_messages: vec![],
            extra_session_fields: Default::default(),
        })
        .connect()
        .await?;

    let _connected = connected;
    server.await?;
    Ok(())
}
