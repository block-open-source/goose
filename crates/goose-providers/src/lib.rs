#[cfg(all(
    feature = "live-websocket",
    not(any(feature = "rustls-tls", feature = "native-tls"))
))]
compile_error!("feature `live-websocket` requires either `rustls-tls` or `native-tls`");

pub mod anthropic;
pub mod api_client;
pub mod azure_foundry;
pub mod databricks;
pub mod databricks_auth;
pub mod databricks_v2;
pub mod google;
pub use goose_provider_types::{
    base, cache_semantics, canonical, context_limit, conversation, documents, errors, formats,
    goose_mode, images, json, model, permission, request_log, retry, thinking, utils,
};
pub mod browser_live_transport;
pub mod declarative;
pub mod http_status;
pub mod live;
#[cfg(feature = "live-websocket")]
pub mod live_transport_websocket;
pub mod live_voice_provider;
#[cfg(feature = "local-inference")]
pub mod local_inference;
pub mod ollama;
pub mod openai;
pub mod openai_compatible;
pub mod openai_live;
#[cfg(feature = "live-websocket")]
pub mod openai_live_voice_provider;
pub mod openrouter;
pub mod openrouter_format;

pub use declarative::declarative_providers::*;

pub mod snowflake;
