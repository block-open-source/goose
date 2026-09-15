//! OpenAI Live low-level API examples.

use anyhow::Result;
use goose_providers::openai_live::{OpenAiLiveClient, OpenAiLiveSessionConfig};

fn config() -> OpenAiLiveSessionConfig {
    OpenAiLiveSessionConfig {
        model: "gpt-live-1".into(),
        instructions: "Be concise.".into(),
        voice: Some("marin".into()),
        input_messages: vec![],
        extra_session_fields: Default::default(),
    }
}

#[cfg(feature = "live-websocket")]
async fn primary_websocket(client: &OpenAiLiveClient) -> Result<()> {
    let mut connected = client.websocket(config()).connect().await?;
    let event = connected.recv().await?;
    println!("{event:?}");
    connected.close().await
}

fn main() {
    let _ = OpenAiLiveClient::from_env;
    #[cfg(feature = "live-websocket")]
    let _ = primary_websocket;
}
