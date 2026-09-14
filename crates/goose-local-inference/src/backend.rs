use rmcp::model::Tool;
use std::any::Any;

use crate::model::ModelSettings;
use goose_provider_types::conversation::message::Message;
use goose_provider_types::errors::ProviderError;
use goose_provider_types::request_log::RequestLogHandle;

use super::{ResolvedModelPaths, StreamSender};

pub(super) trait BackendLoadedModel: Send {
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

#[cfg_attr(not(feature = "mlx"), allow(dead_code))]
pub(super) struct LocalGenerationRequest<'a> {
    pub model_name: String,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [Tool],
    pub settings: &'a ModelSettings,
    pub temperature: Option<f32>,
    pub max_tokens: Option<i32>,
    pub context_limit: usize,
    pub model_load_ms: Option<u64>,
    pub resolved_model: &'a ResolvedModelPaths,
    pub draft_model_path: Option<std::path::PathBuf>,
    pub message_id: &'a str,
    pub tx: &'a StreamSender,
    pub log: &'a mut Option<Box<dyn RequestLogHandle>>,
}

pub(super) trait LocalInferenceBackend: Send + Sync {
    fn id(&self) -> &'static str;

    fn load_model(
        &self,
        model_id: &str,
        resolved: &ResolvedModelPaths,
        settings: &ModelSettings,
    ) -> Result<Box<dyn BackendLoadedModel>, ProviderError>;

    fn generate(
        &self,
        loaded: &mut dyn BackendLoadedModel,
        request: LocalGenerationRequest<'_>,
    ) -> Result<(), ProviderError>;

    fn available_memory_bytes(&self) -> u64;
}
