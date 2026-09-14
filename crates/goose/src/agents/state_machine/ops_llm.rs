//! Goose integration for the reusable inference operation.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use goose_agent::inference::InferenceEffect;
pub use goose_agent::inference::InferenceRunner;
use goose_providers::base::{MessageStream, ModelInfo, Provider};
use goose_providers::conversation::message::Message;
use goose_providers::conversation::token_usage::ProviderUsage;
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;

use crate::agents::extension_manager::{get_tool_owner, recover_mangled_tool_name};
use crate::agents::state_machine::GooseEffect;

pub(super) use goose_agent::inference::{chat_span, record_chat_usage};

pub(super) const ADVERTISED_TOOLS_NOTE: &str = "advertised_tools";
pub(super) const LLM_OPERATION_NAME: &str = "llm";

pub struct GooseInferenceProvider {
    inner: Arc<dyn Provider>,
}

impl GooseInferenceProvider {
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self { inner }
    }
}

impl InferenceEffect for GooseEffect {
    fn record_usage(usage: ProviderUsage) -> Self {
        GooseEffect::RecordUsage(usage)
    }
}

fn enrich_unclaimed_tool_errors(messages: &[Message], tools: &[rmcp::model::Tool]) -> Vec<Message> {
    let mut available_tools = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    available_tools.sort_unstable();
    available_tools.dedup();
    let available_tools = available_tools.join(", ");
    let mut messages = messages.to_vec();
    for message in &mut messages {
        for content in &mut message.content {
            let goose_providers::conversation::message::MessageContent::ToolResponse(response) =
                content
            else {
                continue;
            };
            let Some(metadata) = &mut response.metadata else {
                continue;
            };
            if metadata
                .remove(super::ops_unknown_tool::UNCLAIMED_TOOL_ERROR)
                .is_none()
            {
                continue;
            }
            let Ok(result) = &mut response.tool_result else {
                continue;
            };
            result.content.push(rmcp::model::ContentBlock::text(format!(
                "Available tools: [{available_tools}]."
            )));
        }
    }
    messages
}

fn canonicalize_tool_request_names(
    message: &mut Message,
    advertised_tools: &[(String, Option<String>)],
) {
    for content in &mut message.content {
        let goose_providers::conversation::message::MessageContent::ToolRequest(request) = content
        else {
            continue;
        };
        let Ok(tool_call) = &mut request.tool_call else {
            continue;
        };
        let Some(recovered) = recover_mangled_tool_name(
            &tool_call.name,
            advertised_tools
                .iter()
                .map(|(name, owner)| (name.as_str(), owner.as_deref())),
        ) else {
            continue;
        };
        tool_call.name = recovered.into();
    }
}

#[async_trait]
impl Provider for GooseInferenceProvider {
    fn get_name(&self) -> &str {
        self.inner.get_name()
    }

    fn provider_session_id(&self) -> Option<String> {
        self.inner.provider_session_id()
    }

    async fn resume(&self, session_id: &str) -> Result<(), ProviderError> {
        self.inner.resume(session_id).await
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[rmcp::model::Tool],
    ) -> Result<MessageStream, ProviderError> {
        let messages = enrich_unclaimed_tool_errors(messages, tools);
        let (tools, toolshim_tools, system_prompt) =
            crate::agents::reply_parts::prepare_tools_for_provider(
                tools.to_vec(),
                system.to_string(),
                model_config,
            );
        let advertised_tool_descriptors = tools
            .iter()
            .chain(toolshim_tools.iter())
            .map(|tool| (tool.name.to_string(), get_tool_owner(tool)))
            .collect::<Vec<_>>();
        let mut advertised_tools = advertised_tool_descriptors
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        advertised_tools.sort_unstable();
        advertised_tools.dedup();
        let advertised_tools_note = serde_json::Value::Array(
            advertised_tools
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        );
        let session_id = crate::session_context::current_session_id().unwrap_or_default();
        let stream = crate::agents::reply_parts::stream_response_from_provider(
            self.inner.clone(),
            model_config.clone(),
            &session_id,
            &system_prompt,
            &messages,
            &tools,
            &toolshim_tools,
        )
        .await?;
        Ok(Box::pin(stream.map(move |result| {
            result.map(|(message, usage)| {
                let message = message.map(|mut message| {
                    canonicalize_tool_request_names(&mut message, &advertised_tool_descriptors);
                    if message.role == rmcp::model::Role::Assistant {
                        message.metadata.set_operation_note(
                            LLM_OPERATION_NAME,
                            ADVERTISED_TOOLS_NOTE,
                            advertised_tools_note.clone(),
                        );
                    }
                    message
                });
                (message, usage)
            })
        })))
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        self.inner.get_context_limit(model, override_limit).await
    }

    async fn fetch_model_info(&self, model_name: &str) -> Result<ModelInfo, ProviderError> {
        self.inner.fetch_model_info(model_name).await
    }
}

#[cfg(test)]
mod canonicalization_tests {
    use super::*;
    use rmcp::model::CallToolRequestParams;

    fn request(name: &str) -> Message {
        Message::assistant()
            .with_tool_request("request", Ok(CallToolRequestParams::new(name.to_string())))
    }

    fn tool_name(message: &Message) -> &str {
        message.content[0]
            .as_tool_request()
            .unwrap()
            .tool_call
            .as_ref()
            .unwrap()
            .name
            .as_ref()
    }

    #[test]
    fn canonicalizes_mangled_names_against_advertised_tools() {
        let advertised = vec![("developer__shell".to_string(), None)];
        let mut message = request("developer.shell");

        canonicalize_tool_request_names(&mut message, &advertised);

        assert_eq!(tool_name(&message), "developer__shell");
    }

    #[test]
    fn canonicalizes_owner_qualified_unprefixed_tool_aliases() {
        let advertised = vec![("shell".to_string(), Some("developer".to_string()))];
        let mut message = request("developer.shell");

        canonicalize_tool_request_names(&mut message, &advertised);

        assert_eq!(tool_name(&message), "shell");

        let mut message = request("developer__shell");

        canonicalize_tool_request_names(&mut message, &advertised);

        assert_eq!(tool_name(&message), "shell");
    }

    #[test]
    fn leaves_unrecoverable_names_unmodified() {
        let advertised = vec![("developer__shell".to_string(), None)];
        let mut message = request("developer.shell!");

        canonicalize_tool_request_names(&mut message, &advertised);

        assert_eq!(tool_name(&message), "developer.shell!");
    }
}
