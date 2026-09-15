pub mod adversary_inspector;
pub mod classification_client;
pub mod egress_inspector;
pub mod patterns;
pub mod scanner;
pub mod security_inspector;

use crate::config::Config;
use crate::conversation::message::{Message, ToolRequest};
use crate::permission::permission_judge::PermissionCheckResult;
use anyhow::Result;
use scanner::{PromptInjectionScanner, ScannerSettings};
use std::env;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

pub(crate) fn get_override(env_key: &str) -> Option<bool> {
    env::var(env_key).ok().and_then(|v| match v.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

struct CachedScanner {
    settings: ScannerSettings,
    scanner: Arc<PromptInjectionScanner>,
}

pub struct SecurityManager {
    scanner: RwLock<Option<CachedScanner>>,
    #[cfg(test)]
    enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct SecurityResult {
    pub is_malicious: bool,
    pub confidence: f32,
    pub explanation: String,
    pub should_ask_user: bool,
    pub finding_id: String,
    pub tool_request_id: String,
}

impl SecurityManager {
    pub fn new() -> Self {
        Self {
            scanner: RwLock::new(None),
            #[cfg(test)]
            enabled: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn enabled() -> Self {
        Self {
            scanner: RwLock::new(None),
            enabled: Some(true),
        }
    }

    pub fn is_prompt_injection_detection_enabled(&self) -> bool {
        #[cfg(test)]
        {
            if let Some(enabled) = self.enabled {
                return enabled;
            }
        }
        if let Some(overridden) = get_override("SECURITY_PROMPT_ENABLED_OVERRIDE") {
            return overridden;
        }

        let config = Config::global();
        config
            .get_param::<bool>("SECURITY_PROMPT_ENABLED")
            .unwrap_or(false)
    }

    fn scanner_for_settings(&self, settings: ScannerSettings) -> Arc<PromptInjectionScanner> {
        if let Some(cached) = self
            .scanner
            .read()
            .unwrap()
            .as_ref()
            .filter(|cached| cached.settings == settings)
        {
            return cached.scanner.clone();
        }

        let mut cached = self.scanner.write().unwrap();
        if let Some(current) = cached
            .as_ref()
            .filter(|current| current.settings == settings)
        {
            return current.scanner.clone();
        }

        tracing::info!(
            monotonic_counter.goose.security_command_classifier_enabled =
                if settings.command_enabled() { 1 } else { 0 },
            monotonic_counter.goose.security_prompt_classifier_enabled =
                if settings.prompt_enabled() { 1 } else { 0 },
            "Security classifier configuration"
        );

        let scanner = Arc::new(if settings.ml_enabled() {
            match PromptInjectionScanner::with_ml_detection_for_settings(&settings) {
                Ok(scanner) => {
                    tracing::info!(
                        monotonic_counter.goose.prompt_injection_scanner_enabled = 1,
                        "Security scanner initialized with ML-based detection"
                    );
                    scanner
                }
                Err(error) => {
                    tracing::warn!(
                        "ML scanning requested but failed to initialize. Falling back to pattern-only scanning.\n\nError details:\n{:#}",
                        error
                    );
                    PromptInjectionScanner::new()
                }
            }
        } else {
            tracing::info!(
                monotonic_counter.goose.prompt_injection_scanner_enabled = 1,
                "Security scanner initialized with pattern-based detection only"
            );
            PromptInjectionScanner::new()
        });

        *cached = Some(CachedScanner {
            settings,
            scanner: scanner.clone(),
        });
        scanner
    }

    pub async fn analyze_tool_requests(
        &self,
        tool_requests: &[ToolRequest],
        messages: &[Message],
    ) -> Result<Vec<SecurityResult>> {
        if !self.is_prompt_injection_detection_enabled() {
            tracing::debug!(
                monotonic_counter.goose.prompt_injection_scanner_disabled = 1,
                "Security scanning disabled"
            );
            return Ok(vec![]);
        }

        let scanner = self.scanner_for_settings(ScannerSettings::current());

        let mut results = Vec::new();

        tracing::debug!(
            "Starting security analysis - {} tool requests, {} messages",
            tool_requests.len(),
            messages.len()
        );

        for tool_request in tool_requests.iter() {
            if let Ok(tool_call) = &tool_request.tool_call {
                let analysis_result = scanner
                    .analyze_tool_call_with_context(tool_call, messages)
                    .await?;

                let config_threshold = scanner.get_threshold_from_config();
                let sanitized_explanation = analysis_result.explanation.replace('\n', " | ");

                if analysis_result.is_malicious {
                    let above_threshold = analysis_result.confidence > config_threshold;
                    let finding_id = format!("SEC-{}", Uuid::new_v4().simple());

                    let tool_call_json =
                        serde_json::to_string(&tool_call).unwrap_or_else(|_| "{}".to_string());

                    let action = if above_threshold { "BLOCK" } else { "LOG" };

                    tracing::warn!(
                        monotonic_counter.goose.prompt_injection_finding = 1,
                        security.event_type = "prompt_injection_scan",
                        security.action = action,
                        security.confidence = analysis_result.confidence,
                        security.threshold = config_threshold,
                        security.above_threshold = above_threshold,
                        security.threat_type = "command_injection",
                        security.finding_id = %finding_id,
                        security.explanation = %sanitized_explanation,
                        tool.name = %tool_call.name,
                        tool.request_id = %tool_request.id,
                        tool.call_json = %tool_call_json,
                        "{}",
                        if above_threshold {
                            "prompt injection scan: finding above threshold"
                        } else {
                            "prompt injection scan: finding below threshold"
                        }
                    );
                    if above_threshold {
                        results.push(SecurityResult {
                            is_malicious: analysis_result.is_malicious,
                            confidence: analysis_result.confidence,
                            explanation: analysis_result.explanation,
                            should_ask_user: true, // Always ask user for threats above threshold
                            finding_id,
                            tool_request_id: tool_request.id.clone(),
                        });
                    }
                } else if analysis_result.scanned {
                    let tool_call_json =
                        serde_json::to_string(&tool_call).unwrap_or_else(|_| "{}".to_string());

                    tracing::info!(
                        monotonic_counter.goose.prompt_injection_tool_call_passed = 1,
                        security.event_type = "prompt_injection_scan",
                        security.action = "ALLOW",
                        security.confidence = analysis_result.confidence,
                        security.threshold = config_threshold,
                        security.above_threshold = false,
                        security.threat_type = "command_injection",
                        tool.name = %tool_call.name,
                        tool.request_id = %tool_request.id,
                        tool.call_json = %tool_call_json,
                        "prompt injection scan: tool call passed"
                    );
                }
            }
        }

        tracing::info!(
            monotonic_counter.goose.prompt_injection_analysis_performed = 1,
            security_issues_found = results.len(),
            "Prompt injection detection: Security analysis complete"
        );
        Ok(results)
    }

    pub async fn filter_malicious_tool_calls(
        &self,
        messages: &[Message],
        permission_check_result: &PermissionCheckResult,
        _system_prompt: Option<&str>,
    ) -> Result<Vec<SecurityResult>> {
        let tool_requests: Vec<_> = permission_check_result
            .approved
            .iter()
            .chain(permission_check_result.needs_approval.iter())
            .cloned()
            .collect();

        self.analyze_tool_requests(&tool_requests, messages).await
    }
}

impl Default for SecurityManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolRequestParams;
    use rmcp::object;
    use serde_json::json;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn shell_request() -> ToolRequest {
        ToolRequest {
            id: "security-test".to_string(),
            tool_call: Ok(CallToolRequestParams::new("shell")
                .with_arguments(object!({"command": "echo safe"}))),
            metadata: None,
            tool_meta: None,
        }
    }

    async fn mount_classifier(server: &MockServer, token: &str) {
        Mock::given(method("POST"))
            .and(header("authorization", format!("Bearer {token}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([[{
                "label": "SAFE",
                "score": 1.0
            }]])))
            .mount(server)
            .await;
    }

    async fn assert_authorization(server: &MockServer, expected: &str) {
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                == Some(expected),
            "classifier request used unexpected authorization"
        );
    }

    #[tokio::test]
    async fn prompt_classifier_refreshes_after_disable_and_rotation() {
        let first = MockServer::start().await;
        let second = MockServer::start().await;
        mount_classifier(&first, "first-token").await;
        mount_classifier(&second, "second-token").await;
        let _guard = env_lock::lock_env([
            ("SECURITY_PROMPT_CLASSIFIER_ENABLED", Some("true")),
            ("SECURITY_PROMPT_CLASSIFIER_MODEL", None),
            (
                "SECURITY_PROMPT_CLASSIFIER_ENDPOINT",
                Some(first.uri().as_str()),
            ),
            ("SECURITY_PROMPT_CLASSIFIER_TOKEN", Some("first-token")),
            ("SECURITY_COMMAND_CLASSIFIER_ENABLED", Some("false")),
            ("SECURITY_COMMAND_CLASSIFIER_ENABLED_OVERRIDE", None),
            ("SECURITY_COMMAND_CLASSIFIER_MODEL", None),
            ("SECURITY_COMMAND_CLASSIFIER_ENDPOINT", None),
            ("SECURITY_COMMAND_CLASSIFIER_TOKEN", None),
        ]);
        let manager = SecurityManager::enabled();
        let request = shell_request();
        let messages = [Message::user().with_text("ordinary prompt")];

        manager
            .analyze_tool_requests(std::slice::from_ref(&request), &messages)
            .await
            .unwrap();
        assert_authorization(&first, "Bearer first-token").await;

        std::env::set_var("SECURITY_PROMPT_CLASSIFIER_ENABLED", "false");
        manager
            .analyze_tool_requests(std::slice::from_ref(&request), &messages)
            .await
            .unwrap();
        assert_eq!(first.received_requests().await.unwrap().len(), 1);

        std::env::set_var("SECURITY_PROMPT_CLASSIFIER_ENABLED", "true");
        std::env::set_var("SECURITY_PROMPT_CLASSIFIER_ENDPOINT", second.uri());
        std::env::set_var("SECURITY_PROMPT_CLASSIFIER_TOKEN", "second-token");
        manager
            .analyze_tool_requests(std::slice::from_ref(&request), &messages)
            .await
            .unwrap();

        assert_eq!(first.received_requests().await.unwrap().len(), 1);
        assert_authorization(&second, "Bearer second-token").await;
    }

    #[tokio::test]
    async fn command_classifier_stops_immediately_when_disabled() {
        let server = MockServer::start().await;
        mount_classifier(&server, "command-token").await;
        let _guard = env_lock::lock_env([
            ("SECURITY_PROMPT_CLASSIFIER_ENABLED", Some("false")),
            ("SECURITY_PROMPT_CLASSIFIER_MODEL", None),
            ("SECURITY_PROMPT_CLASSIFIER_ENDPOINT", None),
            ("SECURITY_PROMPT_CLASSIFIER_TOKEN", None),
            ("SECURITY_COMMAND_CLASSIFIER_ENABLED", Some("true")),
            ("SECURITY_COMMAND_CLASSIFIER_ENABLED_OVERRIDE", None),
            ("SECURITY_COMMAND_CLASSIFIER_MODEL", None),
            (
                "SECURITY_COMMAND_CLASSIFIER_ENDPOINT",
                Some(server.uri().as_str()),
            ),
            ("SECURITY_COMMAND_CLASSIFIER_TOKEN", Some("command-token")),
        ]);
        let manager = SecurityManager::enabled();
        let request = shell_request();

        manager
            .analyze_tool_requests(std::slice::from_ref(&request), &[])
            .await
            .unwrap();
        assert_authorization(&server, "Bearer command-token").await;

        std::env::set_var("SECURITY_COMMAND_CLASSIFIER_ENABLED", "false");
        manager
            .analyze_tool_requests(std::slice::from_ref(&request), &[])
            .await
            .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
