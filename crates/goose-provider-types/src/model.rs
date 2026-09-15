use crate::formats::openai::{
    extract_reasoning_effort, is_openai_responses_model, is_xai_reasoning_model,
    supports_xai_reasoning_effort,
};
use crate::thinking::ThinkingEffort;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub const DEFAULT_CONTEXT_LIMIT: usize = 128_000;

/// Request param keys that describe model-family-agnostic reasoning behavior and
/// are therefore safe to carry across a model switch or subagent delegation.
/// Provider-specific keys (e.g. `anthropic_beta`) are deliberately excluded so
/// they can't bleed into a request targeting a different model family.
const INHERITED_SESSION_PARAM_KEYS: &[&str] = &[
    "thinking_effort",
    "thinking_budget",
    "budget_tokens",
    "enable_thinking",
    "preserve_thinking_context",
    "preserve_unsigned_thinking",
];

/// Request params goose consumes itself: formats that forward unknown params into
/// the payload must skip these, or the provider gets an unrecognized wire parameter.
pub fn is_goose_internal_request_param(key: &str) -> bool {
    matches!(
        key,
        "thinking_effort"
            | "disable_prompt_cache"
            | "cache_ttl"
            | "emit_clear_thinking"
            | "preserve_thinking_context"
            | "preserve_unsigned_thinking"
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelConfig {
    pub model_name: String,
    #[serde(skip)]
    pub context_limit: Option<usize>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<i32>,
    pub toolshim: bool,
    pub toolshim_model: Option<String>,
    /// Provider-specific request parameters (e.g., anthropic_beta headers)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_params: Option<HashMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
    /// Per-request HTTP headers attached to outgoing provider calls.
    /// Never serialized into request bodies.
    #[serde(skip)]
    pub request_headers: Option<HashMap<String, String>>,
}

impl<'de> Deserialize<'de> for ModelConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawModelConfig {
            model_name: String,
            #[serde(rename = "context_limit")]
            _context_limit: Option<usize>,
            temperature: Option<f32>,
            max_tokens: Option<i32>,
            toolshim: bool,
            toolshim_model: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            request_params: Option<HashMap<String, Value>>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            reasoning: Option<bool>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            supports_vision: Option<bool>,
        }

        let raw = RawModelConfig::deserialize(deserializer)?;
        let mut config = Self {
            model_name: raw.model_name,
            context_limit: None,
            temperature: raw.temperature,
            max_tokens: raw.max_tokens,
            toolshim: raw.toolshim,
            toolshim_model: raw.toolshim_model,
            request_params: raw.request_params,
            reasoning: raw.reasoning,
            supports_vision: raw.supports_vision,
            request_headers: None,
        };
        config.normalize_effort_suffix();
        Ok(config)
    }
}

impl ModelConfig {
    pub fn new(model_name: impl AsRef<str>) -> Self {
        let mut config = Self {
            model_name: model_name.as_ref().to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        config.normalize_effort_suffix();
        config
    }

    pub fn with_canonical_limits(mut self, provider_name: &str) -> Self {
        // Try canonical lookup with the full model name first, then fall back
        // to the name with reasoning-effort suffixes stripped (e.g.
        // "databricks-gpt-5.4-high" → "databricks-gpt-5.4").
        let canonical =
            crate::canonical::maybe_get_canonical_model(provider_name, &self.model_name).or_else(
                || {
                    let (base, _effort) = extract_reasoning_effort(&self.model_name);
                    if base != self.model_name {
                        crate::canonical::maybe_get_canonical_model(provider_name, &base)
                    } else {
                        None
                    }
                },
            );

        if let Some(canonical) = canonical {
            if self.max_tokens.is_none() {
                self.max_tokens = canonical
                    .limit
                    .output
                    .filter(|&output| output < canonical.limit.context)
                    .map(|output| output as i32);
            }
            if self.reasoning.is_none() {
                self.reasoning = canonical.reasoning;
            }
            if self.supports_vision.is_none() {
                self.supports_vision = Some(
                    canonical
                        .modalities
                        .input
                        .contains(&crate::canonical::Modality::Image),
                )
            }
        }

        self
    }

    pub fn with_context_limit(mut self, limit: Option<usize>) -> Self {
        if limit.is_some() {
            self.context_limit = limit;
        }
        self
    }

    pub fn with_temperature(mut self, temp: Option<f32>) -> Self {
        self.temperature = temp;
        self
    }

    pub fn with_max_tokens(mut self, tokens: Option<i32>) -> Self {
        self.max_tokens = tokens;
        self
    }

    pub fn with_default_context_limit(mut self, limit: Option<usize>) -> Self {
        if self.context_limit.is_none() {
            self.context_limit = limit;
        }
        self
    }

    pub fn with_default_max_tokens(mut self, tokens: Option<i32>) -> Self {
        if self.max_tokens.is_none() {
            self.max_tokens = tokens;
        }
        self
    }

    pub fn with_toolshim(mut self, toolshim: bool) -> Self {
        self.toolshim = toolshim;
        self
    }

    pub fn with_toolshim_model(mut self, model: Option<String>) -> Self {
        self.toolshim_model = model;
        self
    }

    pub fn with_request_headers(mut self, headers: Option<HashMap<String, String>>) -> Self {
        self.request_headers = headers;
        self
    }

    pub fn with_merged_request_params(mut self, params: HashMap<String, Value>) -> Self {
        match self.request_params.as_mut() {
            Some(existing) => {
                for (k, v) in params {
                    existing.insert(k, v);
                }
            }
            None => {
                self.request_params = Some(params);
            }
        }
        self
    }

    pub fn with_thinking_effort(mut self, effort: ThinkingEffort) -> Self {
        let params = self.request_params.get_or_insert_with(HashMap::new);
        params.insert(
            "thinking_effort".to_string(),
            serde_json::json!(effort.to_string()),
        );
        self
    }

    pub fn with_default_thinking_effort(mut self, effort: Option<ThinkingEffort>) -> Self {
        // Guard on raw-param presence rather than parseability: a persisted
        // harness value like "default" doesn't parse into ThinkingEffort but
        // is still an explicit user pick that must not be overwritten.
        if self.request_param::<String>("thinking_effort").is_none() {
            if let Some(effort) = effort {
                self = self.with_thinking_effort(effort);
            }
        }
        self
    }

    pub fn with_vision_support(mut self, supports_vision: bool) -> Self {
        self.supports_vision = Some(supports_vision);
        self
    }

    pub fn with_inherited_session_settings_from(
        mut self,
        previous: Option<&ModelConfig>,
        request_params: Option<HashMap<String, Value>>,
    ) -> Self {
        if let Some(previous_params) = previous.and_then(|p| p.request_params.as_ref()) {
            for key in INHERITED_SESSION_PARAM_KEYS {
                if let Some(value) = previous_params.get(*key) {
                    self.request_params
                        .get_or_insert_with(HashMap::new)
                        .entry(key.to_string())
                        .or_insert_with(|| value.clone());
                }
            }
        }

        if let Some(request_params) = request_params {
            self = self.with_merged_request_params(request_params);
        }

        self
    }

    pub fn context_limit(&self) -> usize {
        self.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT)
    }

    pub fn is_openai_reasoning_model(&self) -> bool {
        is_openai_responses_model(&self.model_name)
    }

    pub fn is_reasoning_model(&self) -> bool {
        if let Some(reasoning) = self.reasoning {
            return reasoning;
        }

        self.is_openai_reasoning_model()
            || self.model_name.to_lowercase().contains("claude")
            || Self::is_gemini3_reasoning_model_name(&self.model_name)
            || is_xai_reasoning_model(&self.model_name)
    }

    fn is_gemini3_reasoning_model_name(model_name: &str) -> bool {
        let lower = model_name.to_lowercase();
        lower.starts_with("gemini-3") || lower.contains("/gemini-3") || lower.contains("-gemini-3")
    }

    pub fn max_output_tokens(&self) -> i32 {
        if let Some(tokens) = self.max_tokens {
            return tokens;
        }

        4_096
    }

    pub fn normalize_effort_suffix(&mut self) {
        if !self.is_openai_reasoning_model() && !supports_xai_reasoning_effort(&self.model_name) {
            return;
        }
        let parts: Vec<&str> = self.model_name.split('-').collect();
        let last = match parts.last() {
            Some(l) => *l,
            None => return,
        };
        let effort = match last {
            "none" => ThinkingEffort::Off,
            "low" => ThinkingEffort::Low,
            "medium" => ThinkingEffort::Medium,
            "high" => ThinkingEffort::High,
            "xhigh" => ThinkingEffort::Max,
            _ => return,
        };
        self.model_name = parts[..parts.len() - 1].join("-");
        let has_explicit_effort = self
            .request_params
            .as_ref()
            .and_then(|p| p.get("thinking_effort"))
            .is_some();
        if !has_explicit_effort {
            let params = self.request_params.get_or_insert_with(HashMap::new);
            params.insert(
                "thinking_effort".to_string(),
                serde_json::json!(effort.to_string()),
            );
        }
    }

    pub fn thinking_effort(&self) -> Option<ThinkingEffort> {
        self.request_param::<String>("thinking_effort")
            .and_then(|s| s.parse::<ThinkingEffort>().ok())
    }

    pub fn with_prompt_cache_disabled(self) -> Self {
        self.with_merged_request_params(HashMap::from([(
            "disable_prompt_cache".to_string(),
            Value::Bool(true),
        )]))
    }

    pub fn prompt_cache_disabled(&self) -> bool {
        self.request_param::<bool>("disable_prompt_cache")
            .unwrap_or(false)
    }

    /// Set the prompt-cache TTL requested from providers that support one
    /// (currently the Anthropic message format). Valid values are "5m" and
    /// "1h"; absent means the provider default (5m).
    pub fn with_cache_ttl(self, ttl: &str) -> Self {
        self.with_merged_request_params(HashMap::from([(
            "cache_ttl".to_string(),
            Value::String(ttl.to_string()),
        )]))
    }

    /// Remove any prompt-cache TTL request parameter. The TTL is
    /// configuration state, not session state: callers that resume a
    /// persisted config drop the stored value and re-derive it from the
    /// current configuration so a clamped run never sticks to the session.
    pub fn without_cache_ttl(mut self) -> Self {
        if let Some(params) = self.request_params.as_mut() {
            params.remove("cache_ttl");
            if params.is_empty() {
                self.request_params = None;
            }
        }
        self
    }

    /// Clamp the prompt-cache TTL back to the provider default (5m).
    /// Burst-only surfaces (headless runs, subagents, scheduled recipes) call
    /// this so a user-level 1h opt-in never pays the 2x cache-write premium on
    /// workloads that finish in one burst and cannot idle.
    pub fn with_cache_ttl_clamped(self) -> Self {
        if self.cache_ttl().is_some_and(|ttl| ttl != "5m") {
            self.with_cache_ttl("5m")
        } else {
            self
        }
    }

    pub fn cache_ttl(&self) -> Option<String> {
        self.request_param::<String>("cache_ttl")
    }

    pub fn request_param<T: for<'de> serde::Deserialize<'de>>(
        &self,
        request_key: &str,
    ) -> Option<T> {
        self.request_params
            .as_ref()
            .and_then(|params| params.get(request_key))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_ttl_round_trips_through_request_params() {
        let config = ModelConfig::new("claude-sonnet-4-5").with_cache_ttl("1h");
        assert_eq!(config.cache_ttl().as_deref(), Some("1h"));
        assert!(ModelConfig::new("claude-sonnet-4-5").cache_ttl().is_none());
    }

    #[test]
    fn cache_ttl_clamp_resets_one_hour_to_default() {
        let config = ModelConfig::new("claude-sonnet-4-5")
            .with_cache_ttl("1h")
            .with_cache_ttl_clamped();
        assert_eq!(config.cache_ttl().as_deref(), Some("5m"));
    }

    #[test]
    fn without_cache_ttl_removes_the_param_and_empty_map() {
        let config = ModelConfig::new("claude-sonnet-4-5")
            .with_cache_ttl("1h")
            .without_cache_ttl();
        assert!(config.cache_ttl().is_none());
        assert!(config.request_params.is_none());
    }

    #[test]
    fn without_cache_ttl_preserves_other_request_params() {
        let config = ModelConfig::new("claude-sonnet-4-5")
            .with_merged_request_params(HashMap::from([(
                "thinking_effort".to_string(),
                serde_json::json!("high"),
            )]))
            .with_cache_ttl("1h")
            .without_cache_ttl();
        assert!(config.cache_ttl().is_none());
        assert_eq!(
            config.request_param::<String>("thinking_effort").as_deref(),
            Some("high")
        );
    }

    #[test]
    fn cache_ttl_clamp_leaves_unset_ttl_absent() {
        let config = ModelConfig::new("claude-sonnet-4-5").with_cache_ttl_clamped();
        assert!(config.cache_ttl().is_none());
    }

    #[test]
    fn request_headers_never_serialize_into_bodies() {
        let config = ModelConfig::new("test-model").with_request_headers(Some(HashMap::from([(
            "queue_threshold".to_string(),
            "500".to_string(),
        )])));

        let serialized = serde_json::to_value(&config).unwrap();
        assert!(serialized.get("request_headers").is_none());
        assert_eq!(
            config
                .request_headers
                .as_ref()
                .unwrap()
                .get("queue_threshold"),
            Some(&"500".to_string())
        );
    }

    mod thinking_effort_tests {
        use super::*;

        fn config_with_params(model_name: &str, params: HashMap<String, Value>) -> ModelConfig {
            ModelConfig::new(model_name).with_merged_request_params(params)
        }

        #[test]
        fn from_request_params() {
            let mut params = HashMap::new();
            params.insert("thinking_effort".to_string(), serde_json::json!("medium"));
            let config = config_with_params("test", params);
            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::Medium));
        }

        #[test]
        fn with_thinking_effort_sets_request_param() {
            let config = ModelConfig::new("test").with_thinking_effort(ThinkingEffort::High);

            assert_eq!(
                config
                    .request_params
                    .as_ref()
                    .and_then(|params| params.get("thinking_effort")),
                Some(&serde_json::json!("high"))
            );
        }

        #[test]
        fn with_default_thinking_effort_preserves_unparseable_raw_param() {
            let config = config_with_params(
                "test",
                HashMap::from([("thinking_effort".to_string(), serde_json::json!("default"))]),
            )
            .with_default_thinking_effort(Some(ThinkingEffort::High));

            assert_eq!(
                config
                    .request_params
                    .as_ref()
                    .and_then(|params| params.get("thinking_effort")),
                Some(&serde_json::json!("default"))
            );
        }

        #[test]
        fn with_default_thinking_effort_applies_when_absent() {
            let config =
                ModelConfig::new("test").with_default_thinking_effort(Some(ThinkingEffort::High));

            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::High));
        }

        #[test]
        fn preserves_explicit_thinking_effort() {
            let previous = config_with_params(
                "previous",
                HashMap::from([("thinking_effort".to_string(), serde_json::json!("high"))]),
            );
            let config = ModelConfig::new("next")
                .with_inherited_session_settings_from(Some(&previous), None);

            assert_eq!(
                config
                    .request_params
                    .as_ref()
                    .and_then(|params| params.get("thinking_effort")),
                Some(&serde_json::json!("high"))
            );
        }

        #[test]
        fn does_not_override_existing_thinking_effort() {
            let previous = config_with_params(
                "previous",
                HashMap::from([("thinking_effort".to_string(), serde_json::json!("high"))]),
            );
            let config = config_with_params(
                "next",
                HashMap::from([("thinking_effort".to_string(), serde_json::json!("low"))]),
            )
            .with_inherited_session_settings_from(Some(&previous), None);

            assert_eq!(
                config
                    .request_params
                    .as_ref()
                    .and_then(|params| params.get("thinking_effort")),
                Some(&serde_json::json!("low"))
            );
        }

        #[test]
        fn inherits_reasoning_controls_but_not_provider_specific_params() {
            let previous = config_with_params(
                "previous",
                HashMap::from([
                    ("budget_tokens".to_string(), serde_json::json!(8192)),
                    (
                        "preserve_thinking_context".to_string(),
                        serde_json::json!(true),
                    ),
                    ("anthropic_beta".to_string(), serde_json::json!("beta")),
                ]),
            );
            let config = ModelConfig::new("next")
                .with_inherited_session_settings_from(Some(&previous), None);

            let params = config.request_params.expect("reasoning controls inherited");
            assert_eq!(params.get("budget_tokens"), Some(&serde_json::json!(8192)));
            assert_eq!(
                params.get("preserve_thinking_context"),
                Some(&serde_json::json!(true))
            );
            assert_eq!(params.get("anthropic_beta"), None);
        }

        #[test]
        fn explicit_request_params_override_preserved_session_settings() {
            let previous = config_with_params(
                "previous",
                HashMap::from([("thinking_effort".to_string(), serde_json::json!("high"))]),
            );
            let config = ModelConfig::new("next").with_inherited_session_settings_from(
                Some(&previous),
                Some(HashMap::from([(
                    "thinking_effort".to_string(),
                    serde_json::json!("low"),
                )])),
            );

            assert_eq!(
                config
                    .request_params
                    .as_ref()
                    .and_then(|params| params.get("thinking_effort")),
                Some(&serde_json::json!("low"))
            );
        }

        #[test]
        fn effort_suffix_stripped_from_model_name() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", None::<&str>),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let config = ModelConfig::new("o3-mini-high");
            assert_eq!(config.model_name, "o3-mini");
            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::High));
        }

        #[test]
        fn none_suffix_stripped_from_model_name() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", Some("high")),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let config = ModelConfig::new("o3-mini-none");
            assert_eq!(config.model_name, "o3-mini");
            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::Off));
        }

        #[test]
        fn xhigh_suffix_stripped_from_model_name() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", Some("low")),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let config = ModelConfig::new("gpt-5.4-xhigh");
            assert_eq!(config.model_name, "gpt-5.4");
            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::Max));
        }

        #[test]
        fn effort_suffix_not_stripped_when_thinking_effort_set() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", None::<&str>),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let mut params = HashMap::new();
            params.insert("thinking_effort".to_string(), serde_json::json!("low"));
            let mut config = ModelConfig::new("o3-mini-high");
            // Suffix was already normalized during new(), but if request_params
            // were set before construction, the suffix would not be stripped.
            // Verify the normalized state:
            assert_eq!(config.model_name, "o3-mini");

            // Now simulate setting explicit effort after construction
            config.request_params = Some(params);
            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::Low));
        }

        #[test]
        fn no_suffix_no_change() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", None::<&str>),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let config = ModelConfig::new("o3-mini");
            assert_eq!(config.model_name, "o3-mini");
        }

        #[test]
        fn non_reasoning_model_suffix_not_stripped() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", None::<&str>),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let config = ModelConfig::new("claude-sonnet-4-high");
            assert_eq!(config.model_name, "claude-sonnet-4-high");
        }

        #[test]
        fn xai_reasoning_effort_suffix_is_normalized() {
            let _guard = env_lock::lock_env([
                ("GOOSE_THINKING_EFFORT", None::<&str>),
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_TEMPERATURE", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
                ("GOOSE_TOOLSHIM", None::<&str>),
                ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ]);
            let config = ModelConfig::new("grok-4.5-high");
            assert_eq!(config.model_name, "grok-4.5");
            assert_eq!(config.thinking_effort(), Some(ThinkingEffort::High));
        }

        #[test]
        fn parse_aliases() {
            assert_eq!("off".parse::<ThinkingEffort>(), Ok(ThinkingEffort::Off));
            assert_eq!(
                "disabled".parse::<ThinkingEffort>(),
                Ok(ThinkingEffort::Off)
            );
            assert_eq!("med".parse::<ThinkingEffort>(), Ok(ThinkingEffort::Medium));
            assert_eq!("max".parse::<ThinkingEffort>(), Ok(ThinkingEffort::Max));
            assert_eq!("xhigh".parse::<ThinkingEffort>(), Ok(ThinkingEffort::Max));
            assert!("invalid".parse::<ThinkingEffort>().is_err());
        }
    }

    mod supports_vision {
        use super::*;

        #[test]
        fn reads_supports_vision_from_config() {
            let config: ModelConfig = serde_json::from_str(
                r#"{"model_name":"gpt-4o","toolshim":false,"supports_vision":true}"#,
            )
            .unwrap();
            assert_eq!(config.supports_vision, Some(true));

            let config: ModelConfig = serde_json::from_str(
                r#"{"model_name":"gpt-4o","toolshim":false,"supports_vision":false}"#,
            )
            .unwrap();
            assert_eq!(config.supports_vision, Some(false));
        }

        #[test]
        fn defaults_supports_vision_to_none_when_absent() {
            let config: ModelConfig =
                serde_json::from_str(r#"{"model_name":"deepseek-v4","toolshim":false}"#).unwrap();
            assert_eq!(config.supports_vision, None);
        }

        #[test]
        fn serializes_supports_vision_only_when_some() {
            let config = ModelConfig::new("gpt-4o").with_vision_support(true);
            let serialized = serde_json::to_value(&config).unwrap();
            assert_eq!(
                serialized.get("supports_vision"),
                Some(&serde_json::Value::Bool(true))
            );

            let config = ModelConfig::new("deepseek-v4");
            let serialized = serde_json::to_value(&config).unwrap();
            assert!(serialized.get("supports_vision").is_none());
        }
    }

    mod with_canonical_limits {
        use super::*;

        #[test]
        fn sets_limits_from_canonical_model() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let config = ModelConfig::new("gpt-4o").with_canonical_limits("openai");
            assert_eq!(config.max_tokens, Some(16_384));
            assert_eq!(config.reasoning, Some(false));
        }

        #[test]
        fn does_not_override_existing_max_tokens() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let mut config = ModelConfig::new("gpt-4o");
            config.max_tokens = Some(1_000);
            let config = config.with_canonical_limits("openai");

            assert_eq!(config.max_tokens, Some(1_000));
        }

        #[test]
        fn skips_canonical_output_limit_when_it_equals_context_limit() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let config = ModelConfig::new("moonshotai/kimi-k2.6").with_canonical_limits("nvidia");
            assert_eq!(config.max_tokens, None);
            assert_eq!(config.max_output_tokens(), 4_096);
        }

        #[test]
        fn resolves_claude_sonnet_5_on_aws_bedrock() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let config = ModelConfig::new("global.anthropic.claude-sonnet-5")
                .with_canonical_limits("aws_bedrock");
            assert_eq!(config.max_tokens, Some(128_000));
            assert_eq!(config.reasoning, Some(true));
        }

        #[test]
        fn unknown_model_leaves_fields_none() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let config = ModelConfig::new("totally-unknown-model").with_canonical_limits("openai");

            assert_eq!(config.context_limit, None);
            assert_eq!(config.max_tokens, None);
            assert_eq!(config.reasoning, None);
        }

        #[test]
        fn resolves_after_stripping_reasoning_effort_suffix() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);

            // "gpt-5.6-sol-xhigh" should resolve via "gpt-5.6-sol"
            let config = ModelConfig::new("gpt-5.6-sol-xhigh").with_canonical_limits("openai");
            assert_eq!(config.max_tokens, Some(128_000));
            assert_eq!(config.reasoning, Some(true));
            let canonical = crate::canonical::maybe_get_canonical_model("openai", "gpt-5.6-sol")
                .expect("gpt-5.6-sol should have canonical metadata");
            assert_eq!(canonical.temperature, Some(false));

            let config = ModelConfig::new("gpt-5.6-sol").with_canonical_limits("chatgpt_codex");
            assert_eq!(config.max_tokens, Some(128_000));
            assert_eq!(config.reasoning, Some(true));
        }

        #[test]
        fn resolves_gpt_6_astra_limits_for_databricks_model_service() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let config = ModelConfig::new("data_workflow_tools.goose.goose-gpt-6-astra")
                .with_canonical_limits("databricks_v2");

            let canonical = crate::canonical::maybe_get_canonical_model(
                "databricks_v2",
                "data_workflow_tools.goose.goose-gpt-6-astra",
            )
            .expect("GPT-6 Astra should have canonical metadata");
            assert_eq!(canonical.limit.context, 1_050_000);
            assert_eq!(canonical.limit.output, Some(128_000));
            assert_eq!(config.max_tokens, Some(128_000));
            assert_eq!(config.reasoning, Some(true));
            assert_eq!(config.supports_vision, Some(true));
        }

        #[test]
        fn fills_supports_vision_from_canonical_model() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            // gpt-4o is a vision model in the canonical catalog (image input modality).
            let config = ModelConfig::new("gpt-4o").with_canonical_limits("openai");
            assert_eq!(config.supports_vision, Some(true));
        }

        #[test]
        fn does_not_override_existing_supports_vision() {
            let _guard = env_lock::lock_env([
                ("GOOSE_MAX_TOKENS", None::<&str>),
                ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ]);
            let config = ModelConfig::new("gpt-4o")
                .with_vision_support(false)
                .with_canonical_limits("openai");
            assert_eq!(config.supports_vision, Some(false));
        }
    }

    mod is_openai_reasoning_model {
        use super::*;

        const ENV_LOCK_KEYS: [(&str, Option<&str>); 5] = [
            ("GOOSE_MAX_TOKENS", None),
            ("GOOSE_TEMPERATURE", None),
            ("GOOSE_CONTEXT_LIMIT", None),
            ("GOOSE_TOOLSHIM", None),
            ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None),
        ];

        #[test]
        fn bare_reasoning_models() {
            let _guard = env_lock::lock_env(ENV_LOCK_KEYS);
            assert!(ModelConfig::new("o1").is_openai_reasoning_model());
            assert!(ModelConfig::new("o1-preview").is_openai_reasoning_model());
            assert!(ModelConfig::new("o3").is_openai_reasoning_model());
            assert!(ModelConfig::new("o3-mini").is_openai_reasoning_model());
            assert!(ModelConfig::new("o4-mini").is_openai_reasoning_model());
            assert!(ModelConfig::new("gpt-5").is_openai_reasoning_model());
            assert!(ModelConfig::new("gpt-5-3-codex").is_openai_reasoning_model());
        }

        #[test]
        fn goose_prefixed_reasoning_models() {
            let _guard = env_lock::lock_env(ENV_LOCK_KEYS);
            assert!(ModelConfig::new("goose-o3-mini").is_openai_reasoning_model());
            assert!(ModelConfig::new("goose-o4-mini").is_openai_reasoning_model());
            assert!(ModelConfig::new("goose-gpt-5").is_openai_reasoning_model());
        }

        #[test]
        fn databricks_prefixed_reasoning_models() {
            let _guard = env_lock::lock_env(ENV_LOCK_KEYS);
            assert!(ModelConfig::new("databricks-o3-mini").is_openai_reasoning_model());
            assert!(ModelConfig::new("databricks-o4-mini").is_openai_reasoning_model());
            assert!(ModelConfig::new("databricks-gpt-5").is_openai_reasoning_model());
        }

        #[test]
        fn non_reasoning_models() {
            let _guard = env_lock::lock_env(ENV_LOCK_KEYS);
            assert!(!ModelConfig::new("claude-sonnet-4").is_openai_reasoning_model());
            assert!(!ModelConfig::new("gpt-4o").is_openai_reasoning_model());
            assert!(!ModelConfig::new("databricks-claude-sonnet-4").is_openai_reasoning_model());
            assert!(!ModelConfig::new("goose-claude-sonnet-4").is_openai_reasoning_model());
            assert!(!ModelConfig::new("llama-3-70b").is_openai_reasoning_model());
        }
    }

    mod is_reasoning_model {
        use super::*;

        const ENV_LOCK_KEYS: [(&str, Option<&str>); 5] = [
            ("GOOSE_MAX_TOKENS", None),
            ("GOOSE_TEMPERATURE", None),
            ("GOOSE_CONTEXT_LIMIT", None),
            ("GOOSE_TOOLSHIM", None),
            ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None),
        ];

        #[test]
        fn includes_reasoning_model_families() {
            let _guard = env_lock::lock_env(ENV_LOCK_KEYS);
            assert!(ModelConfig::new("o3-mini").is_reasoning_model());
            assert!(ModelConfig::new("claude-sonnet-4").is_reasoning_model());
            assert!(ModelConfig::new("gemini-3-pro").is_reasoning_model());
            assert!(ModelConfig::new("grok-4.5").is_reasoning_model());
            assert!(ModelConfig::new("grok-4.20-0309-reasoning").is_reasoning_model());
            assert!(!ModelConfig::new("grok-4.20-0309-non-reasoning").is_reasoning_model());
        }

        #[test]
        fn uses_explicit_metadata_first() {
            let _guard = env_lock::lock_env(ENV_LOCK_KEYS);
            let mut config = ModelConfig::new("provider-alias");
            config.reasoning = Some(true);
            assert!(config.is_reasoning_model());

            let mut config = ModelConfig::new("claude-sonnet-4");
            config.reasoning = Some(false);
            assert!(!config.is_reasoning_model());
        }
    }
}
