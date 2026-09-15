use once_cell::sync::Lazy;
use regex::Regex;

// Patterns for normalizing version numbers and stripping suffixes
static NORMALIZE_VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"-(\d)-(\d)(-|@|$)").unwrap());

static STRIP_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"-latest$").unwrap(),
        Regex::new(r"-\d{8}$").unwrap(),
        Regex::new(r"@\d{8}$").unwrap(),
        Regex::new(r"-\d{4}$").unwrap(),
        Regex::new(r"-\d{4}-\d{2}-\d{2}$").unwrap(),
        Regex::new(r"-bedrock$").unwrap(),
    ]
});

static CLAUDE_PATTERNS: Lazy<Vec<(Regex, Regex, &'static str)>> = Lazy::new(|| {
    ["sonnet", "opus", "haiku", "fable"]
        .iter()
        .map(|&size| {
            (
                Regex::new(&format!("claude-([0-9.-]+)-{}", size)).unwrap(),
                Regex::new(&format!("claude-{}-([0-9.-]+)", size)).unwrap(),
                size,
            )
        })
        .collect()
});

/// Build canonical model name from provider and model identifiers
pub fn canonical_name(provider: &str, model: &str) -> String {
    let model_base = strip_version_suffix(model);
    format!("{}/{}", provider, model_base)
}

pub(crate) fn is_meta_provider(provider: &str) -> bool {
    matches!(
        provider,
        "databricks" | "databricks_v2" | "tetrate" | "bedrock" | "azure" | "azure_foundry"
    )
}

pub fn map_provider_name(provider: &str) -> &str {
    match provider {
        // Goose provider names that differ from models.dev names
        "xai" | "xai_oauth" => "x-ai",
        "azure_openai" | "azure_foundry" => "azure",
        "aws_bedrock" => "amazon-bedrock",
        "gcp_vertex_ai" => "google-vertex",
        "gemini_oauth" => "google",
        "databricks_v2" => "databricks",
        "zhipu" => "zhipuai",
        "novita" => "novita-ai",
        "opencode_go" => "opencode-go",
        "opencode_zen" => "opencode",
        "ollama_cloud" => "ollama-cloud",
        "kimi_code" => "kimi-for-coding",
        _ => provider,
    }
}

/// Try to map a provider/model pair to a canonical model
pub fn map_to_canonical_model(
    provider: &str,
    model: &str,
    registry: &super::CanonicalModelRegistry,
) -> Option<String> {
    let registry_provider = map_provider_name(provider);

    if provider == "gcp_vertex_ai" {
        let normalized_model = strip_version_suffix(model);
        if let Some(canonical) = registry.get(registry_provider, &normalized_model) {
            return Some(canonical.id.clone());
        }
        if let Some(canonical) = registry.get(registry_provider, model) {
            return Some(canonical.id.clone());
        }
        if model.starts_with("gemini-") {
            return None;
        }
    }

    // For normal providers (anthropic, openai, google, openrouter, etc.), just do direct lookup
    if !is_meta_provider(provider) && provider != "gcp_vertex_ai" {
        let normalized_model = strip_version_suffix(model);
        if let Some(canonical) = registry.get(registry_provider, &normalized_model) {
            return Some(canonical.id.clone());
        }
        // Also try original model name
        if let Some(canonical) = registry.get(registry_provider, model) {
            return Some(canonical.id.clone());
        }
        // If direct lookup failed, fall through to inference logic below
    }

    // For hosting/meta-providers (or unknown providers), do string matching magic to figure out the real provider and model
    let model_stripped = strip_common_prefixes(model);

    if let Some(swapped) = swap_claude_word_order(&model_stripped) {
        if let Some(inferred_provider) = infer_provider_from_model(&swapped) {
            let normalized = strip_version_suffix(&swapped);
            if let Some(canonical) = registry.get(inferred_provider, &normalized) {
                return Some(canonical.id.clone());
            }
        }
    }

    if let Some(inferred_provider) = infer_provider_from_model(&model_stripped) {
        let normalized = strip_version_suffix(&model_stripped);
        if let Some(canonical) = registry.get(inferred_provider, &normalized) {
            return Some(canonical.id.clone());
        }
    }

    if let Some(inferred_provider) = infer_provider_from_model(model) {
        let normalized = strip_version_suffix(model);
        if let Some(canonical) = registry.get(inferred_provider, &normalized) {
            return Some(canonical.id.clone());
        }
    }

    if let Some((extracted_provider, extracted_model)) = extract_provider_prefix(&model_stripped) {
        let normalized = strip_version_suffix(extracted_model);
        if let Some(canonical) = registry.get(extracted_provider, &normalized) {
            return Some(canonical.id.clone());
        }
    }

    // Fallback for meta-providers: some native aliases are keyed under the
    // meta-provider itself (e.g. "databricks/databricks-gpt-oss-120b") and do
    // not infer back to a first-party provider. Only try this after inference,
    // so models that DO infer (e.g. databricks-claude-* -> anthropic/*) keep
    // resolving to the richer first-party catalog entry.
    if is_meta_provider(provider) {
        if let Some(canonical) = registry.get(registry_provider, model) {
            return Some(canonical.id.clone());
        }
        let normalized_model = strip_version_suffix(model);
        if let Some(canonical) = registry.get(registry_provider, &normalized_model) {
            return Some(canonical.id.clone());
        }
    }

    None
}

/// Swap word order for Claude models to handle both naming conventions
fn swap_claude_word_order(model: &str) -> Option<String> {
    if !model.starts_with("claude-") {
        return None;
    }

    for (forward_re, reverse_re, size) in CLAUDE_PATTERNS.iter() {
        if let Some(captures) = forward_re.captures(model) {
            let version = &captures[1];
            return Some(format!("claude-{}-{}", size, version));
        }

        if let Some(captures) = reverse_re.captures(model) {
            let version = &captures[1];
            return Some(format!("claude-{}-{}", version, size));
        }
    }

    None
}

/// Infer the real provider from model name patterns
fn infer_provider_from_model(model: &str) -> Option<&'static str> {
    let model_lower = model.to_lowercase();

    if model_lower.contains("claude") {
        return Some("anthropic");
    }

    if model_lower.starts_with("gpt-")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.starts_with("o4")
        || model_lower.starts_with("chatgpt-")
    {
        return Some("openai");
    }

    if model_lower.starts_with("gemini-") || model_lower.starts_with("gemma-") {
        return Some("google");
    }

    if model_lower.contains("llama") {
        return Some("meta-llama");
    }

    if model_lower.starts_with("mistral")
        || model_lower.starts_with("mixtral")
        || model_lower.starts_with("codestral")
        || model_lower.starts_with("ministral")
        || model_lower.starts_with("pixtral")
        || model_lower.starts_with("devstral")
        || model_lower.starts_with("voxtral")
    {
        return Some("mistralai");
    }

    if model_lower.contains("deepseek") {
        return Some("deepseek");
    }

    if model_lower.contains("qwen") {
        return Some("qwen");
    }

    if model_lower.contains("grok") {
        return Some("x-ai");
    }

    if model_lower.contains("jamba") {
        return Some("ai21");
    }

    if model_lower.contains("command") {
        return Some("cohere");
    }

    None
}

/// Strip common prefixes from model names using pattern matching
/// Looks for known model family patterns and strips everything before them
fn strip_common_prefixes(model: &str) -> String {
    let model_patterns = [
        "claude-",
        "gpt-",
        "gemini-",
        "gemma-",
        "o1-",
        "o1",
        "o3-",
        "o3",
        "o4-",
        "llama-",
        "mistral-",
        "mixtral-",
        "chatgpt-",
        "deepseek-",
        "qwen-",
        "grok-",
        "jamba-",
        "command-",
        "codestral",
        "ministral-",
        "pixtral-",
        "devstral-",
    ];

    let mut earliest_pos = None;

    for pattern in &model_patterns {
        if let Some(pos) = model.to_lowercase().find(pattern) {
            if earliest_pos.is_none() || pos < earliest_pos.unwrap() {
                earliest_pos = Some(pos);
            }
        }
    }

    // If we found a pattern, strip everything before it
    if let Some(pos) = earliest_pos {
        return model.get(pos..).unwrap_or(model).to_string();
    }

    model.to_string()
}

/// Try to extract provider prefix from model names like "databricks-meta-llama-3-1-70b"
/// Returns (provider, model) tuple if found
fn extract_provider_prefix(model: &str) -> Option<(&'static str, &str)> {
    let known_providers = [
        "anthropic",
        "openai",
        "google",
        "meta-llama",
        "mistralai",
        "cohere",
        "ai21",
        "amazon",
        "deepseek",
        "qwen",
        "x-ai",
        "nvidia",
        "microsoft",
        "perplexity",
    ];

    for provider in &known_providers {
        let prefix = format!("{}-", provider);
        if model.starts_with(&prefix) {
            if let Some(model_part) = model.strip_prefix(&prefix) {
                return Some((provider, model_part));
            }
        }
    }

    None
}

/// Strip version suffixes from model names and normalize version numbers
pub fn strip_version_suffix(model: &str) -> String {
    let mut result = NORMALIZE_VERSION_RE
        .replace_all(model, "-$1.$2$3")
        .to_string();

    let mut changed = true;
    while changed {
        let before = result.clone();
        for pattern in STRIP_PATTERNS.iter() {
            result = pattern.replace(&result, "").to_string();
        }
        changed = result != before;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_to_canonical_model() {
        let r = super::super::CanonicalModelRegistry::bundled().unwrap();

        // === Direct provider (non-hosting) ===
        assert_eq!(
            map_to_canonical_model("anthropic", "claude-sonnet-4-5-20250929", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("openai", "gpt-4o-latest", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("xai_oauth", "grok-4.5", r),
            Some("x-ai/grok-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("openai", "gpt-4-turbo-2024-04-09", r),
            Some("openai/gpt-4-turbo".to_string())
        );
        assert_eq!(
            map_to_canonical_model("opencode_go", "kimi-k2.6", r),
            Some("opencode-go/kimi-k2.6".to_string())
        );
        assert_eq!(
            map_to_canonical_model("opencode_zen", "kimi-k3", r),
            Some("opencode/kimi-k3".to_string())
        );

        // === OpenRouter ===
        assert_eq!(
            map_to_canonical_model("openrouter", "anthropic/claude-sonnet-4.5", r),
            Some("openrouter/anthropic/claude-sonnet-4.5".to_string())
        );

        // === Anthropic Claude - basic ===
        assert_eq!(
            map_to_canonical_model("databricks", "claude-sonnet-4-5", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "claude-sonnet-4-5-20250929", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "claude-sonnet-4-5-latest", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );

        // 4.x: {version}-{model} → {model}-{version}
        assert_eq!(
            map_to_canonical_model("databricks", "claude-sonnet-4-5", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );

        // 4.x with minor version + prefix stripping
        assert_eq!(
            map_to_canonical_model("databricks", "raml-claude-opus-4-5", r),
            Some("anthropic/claude-opus-4.5".to_string())
        );

        // === Claude with platform suffixes ===
        assert_eq!(
            map_to_canonical_model("databricks", "claude-sonnet-4-5-bedrock", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "goose-claude-sonnet-4-5-bedrock", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("bedrock", "claude-sonnet-4-5", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("aws_bedrock", "global.anthropic.claude-sonnet-5", r),
            Some("amazon-bedrock/global.anthropic.claude-sonnet-5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("anthropic", "claude-sonnet-5", r),
            Some("anthropic/claude-sonnet-5".to_string())
        );

        // === OpenAI GPT ===
        assert_eq!(
            map_to_canonical_model("databricks", "gpt-4o", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "gpt-4o-2024-11-20", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "gpt-4o-latest", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "kgoose-gpt-4o", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("azure", "gpt-4o", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("azure_foundry", "gpt-4o", r),
            Some("openai/gpt-4o".to_string())
        );

        // === OpenAI O-series ===
        assert_eq!(
            map_to_canonical_model("databricks", "goose-o1", r),
            Some("openai/o1".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "kgoose-o3", r),
            Some("openai/o3".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "headless-goose-o3-mini", r),
            Some("openai/o3-mini".to_string())
        );

        // === Google Gemini ===
        assert_eq!(
            map_to_canonical_model("databricks", "gemini-2-5-flash", r),
            Some("google/gemini-2.5-flash".to_string())
        );

        // === Meta Llama ===
        assert_eq!(
            map_to_canonical_model("databricks", "meta-llama-3-3-70b-instruct", r),
            Some("meta-llama/llama-3.3-70b-instruct".to_string())
        );

        // === Mistral variants ===
        assert_eq!(
            map_to_canonical_model("databricks", "codestral", r),
            Some("mistralai/codestral".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "ministral-3b", r),
            Some("mistralai/ministral-3b".to_string())
        );

        // === DeepSeek ===
        assert_eq!(
            map_to_canonical_model("databricks", "databricks-deepseek-v4-flash", r),
            Some("deepseek/deepseek-v4-flash".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "deepseek-v4-pro", r),
            Some("deepseek/deepseek-v4-pro".to_string())
        );

        // === Grok (X.AI) ===
        assert_eq!(
            map_to_canonical_model("databricks", "grok-4.3", r),
            Some("x-ai/grok-4.3".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "databricks-grok-4.3", r),
            Some("x-ai/grok-4.3".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "kgoose-grok-4.3", r),
            Some("x-ai/grok-4.3".to_string())
        );

        // === Cohere Command ===
        // Note: version suffix "-2024" is stripped by canonical_name
        assert_eq!(
            map_to_canonical_model("databricks", "command-r-plus-08-2024", r),
            Some("cohere/command-r-plus-08".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "goose-command-r-08-2024", r),
            Some("cohere/command-r-08".to_string())
        );

        // === Provider-prefixed extraction ===
        assert_eq!(
            map_to_canonical_model("databricks", "anthropic-claude-sonnet-4-5", r),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "openai-gpt-4o", r),
            Some("openai/gpt-4o".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "google-gemini-2-5-flash", r),
            Some("google/gemini-2.5-flash".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "mistralai-codestral", r),
            Some("mistralai/codestral".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "deepseek-deepseek-v4-flash", r),
            Some("deepseek/deepseek-v4-flash".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks", "x-ai-grok-4.3", r),
            Some("x-ai/grok-4.3".to_string())
        );

        // === Zhipu AI ===
        assert_eq!(
            map_to_canonical_model("zhipu", "glm-4.7", r),
            Some("zhipuai/glm-4.7".to_string())
        );
        assert_eq!(
            map_to_canonical_model("zhipu", "glm-5", r),
            Some("zhipuai/glm-5".to_string())
        );

        // === Kimi Code ===
        assert_eq!(
            map_to_canonical_model("kimi_code", "kimi-for-coding", r),
            Some("kimi-for-coding/kimi-for-coding".to_string())
        );
        assert_eq!(
            map_to_canonical_model("kimi_code", "kimi-for-coding-highspeed", r),
            Some("kimi-for-coding/kimi-for-coding-highspeed".to_string())
        );

        // === GCP Vertex AI ===
        assert_eq!(
            map_to_canonical_model("gcp_vertex_ai", "gemini-2.5-flash", r),
            Some("google-vertex/gemini-2.5-flash".to_string())
        );
        assert_eq!(
            map_to_canonical_model("gcp_vertex_ai", "gemini-2.5-pro", r),
            Some("google-vertex/gemini-2.5-pro".to_string())
        );
        assert_eq!(
            map_to_canonical_model("gcp_vertex_ai", "claude-sonnet-4@20250514", r),
            Some("google-vertex/claude-sonnet-4".to_string())
        );
        assert_eq!(
            map_to_canonical_model("gcp_vertex_ai", "claude-sonnet-4-5@20250929", r),
            Some("google-vertex/claude-sonnet-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("gcp_vertex_ai", "claude-opus-4-5@20251101", r),
            Some("google-vertex/claude-opus-4.5".to_string())
        );
        assert_eq!(
            map_to_canonical_model("gcp_vertex_ai", "claude-haiku-4-5@20251001", r),
            Some("google-vertex/claude-haiku-4.5".to_string())
        );
    }

    // Databricks-native open-weight ids are keyed under the meta-provider itself
    // (e.g. "databricks/databricks-gpt-oss-120b") and do not infer back to another
    // provider, so they must resolve via the direct meta-provider lookup. These
    // particular ids are unversioned, so the assertions are not catalog-version brittle.
    #[test]
    fn test_databricks_native_open_weight_ids_resolve() {
        let r = super::super::CanonicalModelRegistry::bundled().unwrap();

        assert_eq!(
            map_to_canonical_model("databricks_v2", "databricks-gpt-oss-120b", r),
            Some("databricks/databricks-gpt-oss-120b".to_string())
        );
        assert_eq!(
            map_to_canonical_model("databricks_v2", "databricks-gpt-oss-20b", r),
            Some("databricks/databricks-gpt-oss-20b".to_string())
        );
        // Legacy provider name resolves identically.
        assert_eq!(
            map_to_canonical_model("databricks", "databricks-gpt-oss-120b", r),
            Some("databricks/databricks-gpt-oss-120b".to_string())
        );

        // Regression guard: the meta-provider lookup must remain a *fallback*
        // after inference. databricks-claude-* aliases infer back to the richer
        // first-party "anthropic/*" entry (which carries thinking_mode used for
        // adaptive thinking), not the metadata-poor "databricks/databricks-*" one.
        assert_eq!(
            map_to_canonical_model("databricks", "databricks-claude-opus-4-7", r),
            Some("anthropic/claude-opus-4.7".to_string())
        );
    }
}
