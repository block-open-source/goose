pub mod catalog;
mod model;
mod name_builder;
mod registry;

pub use model::{CanonicalModel, Limit, Modalities, Modality, Pricing, ThinkingMode};
pub use name_builder::{
    canonical_name, map_provider_name, map_to_canonical_model, strip_version_suffix,
};
pub use registry::CanonicalModelRegistry;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelMapping {
    pub provider_model: String,
    pub canonical_model: String,
}

impl ModelMapping {
    pub fn new(provider_model: impl Into<String>, canonical_model: impl Into<String>) -> Self {
        Self {
            provider_model: provider_model.into(),
            canonical_model: canonical_model.into(),
        }
    }
}

/// Return recommended model names for a provider using only the bundled canonical registry.
///
/// This avoids network calls by looking up all known models for the provider,
/// filtering to text-input + tool-calling models, and sorting by release date.
/// The returned names are the canonical short names (e.g. "claude-sonnet-4.5").
///
/// TODO: This trades speed for correctness — the canonical registry may not perfectly
/// match what the provider API returns (new models not yet in the registry, deprecated
/// models still listed, or locally-installed models for providers like Ollama). Consider
/// whether to reconcile with a live API call in the background.
pub fn recommended_models_from_registry(provider: &str) -> Vec<String> {
    let registry = match CanonicalModelRegistry::bundled() {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    let registry_provider = map_provider_name(provider);
    let all = registry.get_all_models_for_provider(registry_provider);

    let mut models_with_dates: Vec<(String, Option<String>)> = all
        .iter()
        .filter(|m| m.modalities.input.contains(&Modality::Text) && m.tool_call)
        .filter(|m| match registry_provider {
            "openai" => !m.id.contains("realtime"),
            "google" => !m.id.contains("deep-research") && !m.id.contains("live"),
            _ => true,
        })
        .filter_map(|m| {
            let (_, name) = m.id.split_once('/')?;
            Some((
                provider_wire_name(provider, &m.id, name),
                m.release_date.clone(),
            ))
        })
        .collect();

    if provider == "xai" {
        models_with_dates.extend(
            mapping_report()["all_mappings"][provider]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|mapping| mapping["provider_model"].as_str())
                .map(|name| (name.to_string(), None)),
        );
    }

    models_with_dates.sort_by(|a, b| {
        let date_order = match (&a.1, &b.1) {
            (Some(date_a), Some(date_b)) => date_b.cmp(date_a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        date_order.then_with(|| a.0.cmp(&b.0))
    });

    let mut seen = std::collections::HashSet::new();
    models_with_dates.retain(|(name, _)| seen.insert(name.clone()));
    models_with_dates
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn mapping_report() -> &'static serde_json::Value {
    static REPORT: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(|| {
        serde_json::from_str(include_str!("canonical/data/canonical_mapping_report.json"))
            .expect("bundled canonical mapping report must be valid")
    });
    &REPORT
}

pub fn provider_wire_name(provider: &str, canonical_id: &str, canonical_name: &str) -> String {
    if map_provider_name(provider) == "anthropic" {
        return dotted_version_to_dash(canonical_name);
    }

    let mapped = mapping_report()["all_mappings"][provider]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mapping| mapping["canonical_model"].as_str() == Some(canonical_id))
        .filter_map(|mapping| mapping["provider_model"].as_str())
        .min_by_key(|name| (!name.ends_with("-latest"), name.len(), *name));
    mapped
        .or_else(|| {
            mapping_report()["unmapped_models"]
                .as_array()?
                .iter()
                .filter(|model| model["provider"].as_str() == Some(provider))
                .filter_map(|model| model["model"].as_str())
                .find(|name| name.strip_suffix("-latest") == Some(canonical_name))
        })
        .unwrap_or(canonical_name)
        .to_string()
}

fn dotted_version_to_dash(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'.'
            && i > 0
            && i + 1 < bytes.len()
            && bytes[i - 1].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
        {
            out.push('-');
        } else {
            out.push(b as char);
        }
    }
    out
}

/// Catalog pricing is not valid for local inference: models served via ollama or a
/// local runtime are actually free to run, so any catalog price would be misleading.
///
/// Azure Foundry is different: it's a meta-provider that proxies models from third-party
/// providers (Anthropic, OpenAI, Meta, etc.). `map_to_canonical_model` already infers the
/// real underlying provider from the model name (e.g. "claude-sonnet-5" -> anthropic,
/// "gpt-5" -> openai) and resolves its public catalog price. That price is a reasonable
/// estimate of the real cost even though it's not guaranteed to match exactly, since Azure
/// billing can vary by deployment region, SKU, offer, and contract/discounts. Callers should
/// treat this as `CostSource::Estimated` rather than `CostSource::ProviderReported`.
fn should_clear_catalog_pricing(provider: &str) -> bool {
    matches!(provider, "ollama" | "local")
}

pub fn maybe_get_canonical_model(provider: &str, model: &str) -> Option<CanonicalModel> {
    let registry = CanonicalModelRegistry::bundled().ok()?;

    let canonical_id = map_to_canonical_model(provider, model, registry)?;
    let mut canonical = if let Some((canon_provider, canon_model)) = canonical_id.split_once('/') {
        registry.get(canon_provider, canon_model).cloned()?
    } else {
        return None;
    };

    if should_clear_catalog_pricing(provider) {
        canonical.cost = Pricing::default();
    } else if name_builder::is_meta_provider(provider) && canonical.cost.has_no_usable_rate() {
        // A meta-provider model can infer to a first-party catalog entry that carries literal
        // 0.0 prices (open-weights publishers such as meta-llama do). The host's own catalog
        // row carries the rate it actually charges to proxy that model, so prefer it. Where
        // there is no such row, report nothing: billing paid proxied inference as free is
        // worse than showing no estimate at all.
        canonical.cost = host_catalog_pricing(provider, model, registry).unwrap_or_default();
    }

    Some(canonical)
}

/// Pricing from the meta-provider's own catalog rows (`azure/*`, `databricks/*`,
/// `amazon-bedrock/*`), which price proxied inference directly rather than by inferring the
/// upstream publisher. Only the rate is taken: model identity and capabilities stay on the
/// canonical entry that `map_to_canonical_model` resolved.
fn host_catalog_pricing(
    provider: &str,
    model: &str,
    registry: &CanonicalModelRegistry,
) -> Option<Pricing> {
    let host = name_builder::map_provider_name(provider);
    let stripped = name_builder::strip_version_suffix(model);
    let cost = registry
        .get(host, &stripped)
        .or_else(|| registry.get(host, model))
        .or_else(|| registry.get(host, &stripped.to_ascii_lowercase()))?
        .cost
        .clone();
    (!cost.has_no_usable_rate()).then_some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_models_have_zero_cost() {
        // "mistral-nemo" resolves to mistralai/mistral-nemo which has non-zero cloud pricing.
        // When accessed via ollama, cost must be zeroed out.
        let canonical = maybe_get_canonical_model("ollama", "mistral-nemo")
            .expect("mistral-nemo should resolve via ollama");
        assert_eq!(canonical.cost.input, None);
        assert_eq!(canonical.cost.output, None);
        assert!(
            canonical.limit.context > 0,
            "context limit should be preserved"
        );
    }

    #[test]
    fn azure_foundry_models_use_inferred_provider_pricing() {
        let canonical = maybe_get_canonical_model("azure_foundry", "gpt-5")
            .expect("gpt-5 should resolve through the Azure catalog");
        assert_eq!(canonical.limit.context, 400_000);
        let openai = maybe_get_canonical_model("openai", "gpt-5")
            .expect("gpt-5 should resolve for its first-party provider");
        assert_eq!(canonical.cost.input, openai.cost.input);
        assert_eq!(canonical.cost.output, openai.cost.output);
        assert!(canonical.cost.input.is_some_and(|price| price > 0.0));
        assert!(canonical.cost.output.is_some_and(|price| price > 0.0));
    }

    #[test]
    fn meta_provider_zero_priced_inference_prefers_the_host_catalog_rate() {
        // "llama-3.3-70b-instruct" infers to meta-llama/llama-3.3-70b-instruct, priced 0.0/0.0
        // because the weights are free to download — but Azure bills to serve them. The
        // azure/llama-3.3-70b-instruct row carries the rate Azure actually charges.
        let canonical = maybe_get_canonical_model("azure_foundry", "llama-3.3-70b-instruct")
            .expect("llama-3.3-70b-instruct should resolve");
        assert_eq!(canonical.cost.input, Some(0.71));
        assert_eq!(canonical.cost.output, Some(0.71));
        assert!(canonical.limit.context > 0);
    }

    #[test]
    fn meta_provider_zero_priced_inference_reports_no_cost_without_a_host_rate() {
        // Databricks bills for llama-3.3-70b-instruct but publishes no catalog row for it.
        // Reporting nothing beats reporting the publisher's 0.0/0.0 as if it were free.
        let canonical = maybe_get_canonical_model("databricks", "llama-3.3-70b-instruct")
            .expect("llama-3.3-70b-instruct should resolve");
        assert_eq!(canonical.cost.input, None);
        assert_eq!(canonical.cost.output, None);
        assert!(canonical.limit.context > 0);
    }

    #[test]
    fn cloud_provider_retains_cost() {
        let canonical = maybe_get_canonical_model("anthropic", "claude-sonnet-4-5-20250929")
            .expect("claude-sonnet-4.5 should resolve");
        assert!(canonical.cost.input.is_some());
        assert!(canonical.cost.output.is_some());
    }

    #[test]
    fn anthropic_opus_5_resolves() {
        let canonical = maybe_get_canonical_model("anthropic", "claude-opus-5")
            .expect("claude-opus-5 should resolve");
        assert_eq!(canonical.limit.context, 1_000_000);
        assert_eq!(canonical.limit.output, Some(128_000));
        assert!(canonical.cost.input.is_some());
        assert!(canonical.cost.output.is_some());
    }

    #[test]
    fn kimi_code_k3_resolves_with_reasoning_and_context_limit() {
        let canonical = maybe_get_canonical_model("kimi_code", "k3")
            .expect("kimi_code/k3 should resolve via kimi-for-coding provider mapping");
        assert_eq!(canonical.limit.context, 1_048_576);
        assert_eq!(canonical.reasoning, Some(true));
        assert_eq!(canonical.temperature, Some(false));
    }

    #[test]
    fn recommended_google_models_use_generate_content() {
        let models = recommended_models_from_registry("google");
        assert!(!models.iter().any(|model| model.contains("deep-research")));
        assert!(!models.iter().any(|model| model.contains("live")));
        assert!(models.iter().any(|model| model == "gemini-2.5-pro"));
    }

    #[test]
    fn anthropic_wire_name_uses_dashed_versions() {
        assert_eq!(
            provider_wire_name(
                "anthropic",
                "anthropic/claude-sonnet-4.5",
                "claude-sonnet-4.5"
            ),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            provider_wire_name("anthropic", "anthropic/claude-opus-5", "claude-opus-5"),
            "claude-opus-5"
        );
        assert_eq!(
            provider_wire_name("openai", "openai/gpt-5.2", "gpt-5.2"),
            "gpt-5.2"
        );
        assert_eq!(
            provider_wire_name("openai", "openai/gpt-5.2-chat", "gpt-5.2-chat"),
            "gpt-5.2-chat-latest"
        );
        assert_eq!(
            provider_wire_name("openai", "openai/gpt-5.3-chat", "gpt-5.3-chat"),
            "gpt-5.3-chat-latest"
        );
    }

    #[test]
    fn xai_models_include_provider_inventory_aliases() {
        let models = recommended_models_from_registry("xai");
        assert!(models.iter().any(|model| model == "grok-3"));
        assert!(models.iter().any(|model| model == "grok-3-mini"));
        assert!(models.iter().any(|model| model == "grok-4-0709"));
        assert!(models.iter().any(|model| model == "grok-code-fast-1"));
    }
}
