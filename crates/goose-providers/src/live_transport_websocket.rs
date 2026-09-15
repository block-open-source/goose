//! Native WebSocket transport for OpenAI Live sessions.

use crate::{live::LiveTransport, openai_live::OpenAiLiveWebSocketRequest};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{stream::SplitSink, stream::SplitStream, SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    sync::Mutex,
    time::{timeout, Duration},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type SocketSink = SplitSink<Socket, Message>;
type SocketStream = SplitStream<Socket>;

pub struct WebSocketLiveTransport {
    sink: Mutex<SocketSink>,
    stream: Mutex<SocketStream>,
}

impl WebSocketLiveTransport {
    pub async fn connect(request: OpenAiLiveWebSocketRequest) -> Result<Self> {
        let mut websocket_request = request.endpoint.into_client_request()?;
        for (name, value) in request.headers {
            websocket_request.headers_mut().insert(
                http::header::HeaderName::try_from(name)?,
                http::header::HeaderValue::try_from(value)?,
            );
        }
        let (socket, _) = timeout(CONNECT_TIMEOUT, connect_async(websocket_request))
            .await
            .context("live WebSocket connection timed out")?
            .context("live WebSocket connection failed")?;
        let (mut sink, stream) = socket.split();
        timeout(CONNECT_TIMEOUT, async {
            for message in request.initial_messages {
                sink.send(Message::Text(message.to_string().into())).await?;
            }
            Result::<()>::Ok(())
        })
        .await
        .context("live WebSocket initialization timed out")??;
        Ok(Self {
            sink: Mutex::new(sink),
            stream: Mutex::new(stream),
        })
    }
}

#[async_trait]
impl LiveTransport for WebSocketLiveTransport {
    async fn send(&self, message: Value) -> Result<()> {
        self.sink
            .lock()
            .await
            .send(Message::Text(message.to_string().into()))
            .await?;
        Ok(())
    }

    async fn receive(&self) -> Result<Option<Value>> {
        let stream = &mut *self.stream.lock().await;
        while let Some(message) = stream.next().await {
            match message? {
                Message::Text(text) => return Ok(Some(serde_json::from_str(&text)?)),
                Message::Binary(bytes) => return Ok(Some(serde_json::from_slice(&bytes)?)),
                Message::Close(_) => return Ok(None),
                _ => {}
            }
        }
        Ok(None)
    }

    async fn close(&self) -> Result<()> {
        self.sink.lock().await.close().await?;
        Ok(())
    }
}
