use crate::backend::LocalInferenceBackend;
use crate::model::ModelSettings;
use crate::multimodal::ExtractedImage;
use goose_provider_types::errors::ProviderError;
use goose_provider_types::request_log::{LoggerHandleExt, RequestLogHandle};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{AddBos, ChatTemplateResult, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::mtmd::{MtmdBitmap, MtmdContext, MtmdInputText};
use llama_cpp_2::openai::OpenAIChatTemplateParams;
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;

use super::super::StreamSender;
use super::LlamaCppBackend;

pub(super) struct GenerationContext<'a> {
    pub loaded: &'a LoadedModel,
    pub backend: &'a LlamaCppBackend,
    pub template: &'a LlamaChatTemplate,
    pub settings: &'a ModelSettings,
    pub context_limit: usize,
    pub model_name: String,
    pub message_id: &'a str,
    pub tx: &'a StreamSender,
    pub log: &'a mut Option<Box<dyn RequestLogHandle>>,
    pub images: &'a [ExtractedImage],
}

pub(super) struct LoadedModel {
    pub model: LlamaModel,
    pub templates: LoadedChatTemplates,
    /// Multimodal context for vision models. None for text-only models.
    pub mtmd_ctx: Option<MtmdContext>,
}

pub(super) struct LoadedChatTemplates {
    pub default: Option<LlamaChatTemplate>,
    pub tool_use: Option<LlamaChatTemplate>,
    pub force_default: bool,
}

pub(super) struct PreparedGeneration<'model> {
    pub template_result: ChatTemplateResult,
    pub llama_ctx: llama_cpp_2::context::LlamaContext<'model>,
    pub prompt_token_count: usize,
    pub effective_ctx: usize,
}

pub(super) struct StopSuffixTrimmer {
    pending: String,
    stops: Vec<String>,
}

impl StopSuffixTrimmer {
    pub(super) fn new(stops: &[String]) -> Self {
        Self {
            pending: String::new(),
            stops: stops
                .iter()
                .filter(|stop| !stop.is_empty())
                .cloned()
                .collect(),
        }
    }

    pub(super) fn push(&mut self, chunk: &str) -> (String, bool) {
        if self.stops.is_empty() {
            return (chunk.to_string(), false);
        }

        self.pending.push_str(chunk);

        if let Some(stop) = self
            .stops
            .iter()
            .filter(|stop| self.pending.ends_with(stop.as_str()))
            .max_by_key(|stop| stop.len())
        {
            let emit_len = self.pending.len() - stop.len();
            let _stop = self.pending.split_off(emit_len);
            let emit = std::mem::take(&mut self.pending);
            return (emit, true);
        }

        let hold_len = self
            .pending
            .char_indices()
            .map(|(idx, _)| idx)
            .chain(std::iter::once(self.pending.len()))
            .filter(|idx| {
                self.pending
                    .get(*idx..)
                    .is_some_and(|suffix| self.stops.iter().any(|stop| stop.starts_with(suffix)))
            })
            .map(|idx| self.pending.len() - idx)
            .max()
            .unwrap_or(0);

        let emit_len = self.pending.len() - hold_len;
        let keep = self.pending.split_off(emit_len);
        let emit = std::mem::replace(&mut self.pending, keep);
        (emit, false)
    }

    pub(super) fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }
}

/// Estimate the maximum context length that can fit in available accelerator/CPU
/// memory based on the model's KV cache requirements.
///
/// Returns `None` if the model architecture values are unavailable.
pub(super) fn estimate_max_context_for_memory(
    model: &LlamaModel,
    backend: &LlamaCppBackend,
    mmproj_overhead_bytes: u64,
) -> Option<usize> {
    let raw_available = backend.available_memory_bytes();
    if raw_available == 0 {
        return None;
    }
    let available = raw_available.saturating_sub(mmproj_overhead_bytes);

    // Reserve memory for computation scratch buffers (attention, etc.) and other overhead.
    // The compute buffer can be 40-50% of the KV cache size for large models, so we
    // conservatively use only half the available memory for the KV cache.
    let usable = (available as f64 * 0.5) as u64;

    let n_layer = model.n_layer() as u64;
    let n_head_kv = model.n_head_kv() as u64;
    let n_head = model.n_head() as u64;
    let n_embd = model.n_embd() as u64;

    if n_head == 0 || n_layer == 0 || n_head_kv == 0 || n_embd == 0 {
        return None;
    }

    // For MLA (Multi-head Latent Attention) models like DeepSeek/GLM, the actual KV cache
    // dimensions differ from n_head_kv * head_dim. Read the true dimensions from GGUF metadata.
    let arch = model
        .meta_val_str("general.architecture")
        .unwrap_or_default();
    let head_dim = n_embd / n_head;
    let k_per_head = model
        .meta_val_str(&format!("{arch}.attention.key_length"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(head_dim);
    let v_per_head = model
        .meta_val_str(&format!("{arch}.attention.value_length"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(head_dim);

    // Total KV dimensions across all KV heads, times n_layer, times 2 bytes (f16) per element
    let bytes_per_token = (k_per_head + v_per_head) * n_head_kv * n_layer * 2;

    if bytes_per_token == 0 {
        return None;
    }

    Some((usable / bytes_per_token) as usize)
}

pub(super) fn context_cap(
    settings: &crate::model::ModelSettings,
    context_limit: usize,
    n_ctx_train: usize,
    memory_max_ctx: Option<usize>,
) -> usize {
    if let Some(ctx_size) = settings.context_size {
        return ctx_size as usize;
    }

    let limit = if context_limit > 0 {
        context_limit
    } else {
        n_ctx_train
    };

    match memory_max_ctx {
        Some(mem_max) if mem_max < limit => {
            tracing::info!(
                "Capping context from {} to {} based on available memory",
                limit,
                mem_max,
            );
            mem_max
        }
        _ => limit,
    }
}

pub(super) fn effective_context_size(
    prompt_token_count: usize,
    settings: &crate::model::ModelSettings,
    context_limit: usize,
    n_ctx_train: usize,
    memory_max_ctx: Option<usize>,
) -> usize {
    let limit = context_cap(settings, context_limit, n_ctx_train, memory_max_ctx);
    let min_generation_headroom = 512;
    if prompt_token_count + min_generation_headroom > limit {
        tracing::warn!(
            "Prompt ({} tokens) + minimum headroom ({}) exceeds context limit ({})",
            prompt_token_count,
            min_generation_headroom,
            limit,
        );
    }
    limit
}

pub(super) fn build_context_params(
    ctx_size: u32,
    settings: &crate::model::ModelSettings,
) -> LlamaContextParams {
    let mut params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(ctx_size));

    if let Some(n_batch) = settings.n_batch {
        params = params.with_n_batch(n_batch);
    }
    if let Some(n_threads) = settings.n_threads {
        params = params.with_n_threads(n_threads);
        params = params.with_n_threads_batch(n_threads);
    }
    if let Some(flash_attn) = settings.flash_attention {
        let policy = if flash_attn { 1 } else { 0 };
        params = params.with_flash_attention_policy(policy);
    }

    params
}

pub(super) fn build_sampler(settings: &crate::model::ModelSettings) -> LlamaSampler {
    use crate::model::SamplingConfig;

    let has_penalties = settings.repeat_penalty != 1.0
        || settings.frequency_penalty != 0.0
        || settings.presence_penalty != 0.0;

    let mut samplers: Vec<LlamaSampler> = Vec::new();

    if has_penalties {
        samplers.push(LlamaSampler::penalties(
            settings.repeat_last_n,
            settings.repeat_penalty,
            settings.frequency_penalty,
            settings.presence_penalty,
        ));
    }

    match &settings.sampling {
        SamplingConfig::Greedy => {
            samplers.push(LlamaSampler::greedy());
        }
        SamplingConfig::Temperature {
            temperature,
            top_k,
            top_p,
            min_p,
            seed,
        } => {
            samplers.push(LlamaSampler::top_k(*top_k));
            samplers.push(LlamaSampler::top_p(*top_p, 1));
            samplers.push(LlamaSampler::min_p(*min_p, 1));
            samplers.push(LlamaSampler::temp(*temperature));
            samplers.push(LlamaSampler::dist(seed.unwrap_or(0)));
        }
        SamplingConfig::MirostatV2 { tau, eta, seed } => {
            samplers.push(LlamaSampler::mirostat_v2(seed.unwrap_or(0), *tau, *eta));
        }
    }

    if samplers.len() == 1 {
        samplers.pop().unwrap()
    } else {
        LlamaSampler::chain_simple(samplers)
    }
}

/// Validate prompt tokens against memory limits and compute the effective
/// context size. Returns `(prompt_token_count, effective_ctx)`.
pub(super) fn validate_and_compute_context(
    loaded: &LoadedModel,
    backend: &LlamaCppBackend,
    prompt_token_count: usize,
    context_limit: usize,
    settings: &crate::model::ModelSettings,
) -> Result<(usize, usize), ProviderError> {
    let n_ctx_train = loaded.model.n_ctx_train() as usize;
    let mmproj_overhead = if loaded.mtmd_ctx.is_some() {
        settings.mmproj_size_bytes
    } else {
        0
    };
    let memory_max_ctx = estimate_max_context_for_memory(&loaded.model, backend, mmproj_overhead);
    let effective_ctx = effective_context_size(
        prompt_token_count,
        settings,
        context_limit,
        n_ctx_train,
        memory_max_ctx,
    );
    if let Some(mem_max) = memory_max_ctx {
        if prompt_token_count > mem_max {
            return Err(ProviderError::ContextLengthExceeded(format!(
                "Prompt ({} tokens) exceeds estimated memory capacity ({} tokens). \
                 Try a smaller model or reduce conversation length.",
                prompt_token_count, mem_max,
            )));
        }
    }
    if prompt_token_count >= effective_ctx {
        return Err(ProviderError::ContextLengthExceeded(format!(
            "Prompt ({} tokens) exceeds context limit ({} tokens). \
             Try reducing conversation length.",
            prompt_token_count, effective_ctx,
        )));
    }
    Ok((prompt_token_count, effective_ctx))
}

/// Create a llama context and prefill (decode) all prompt tokens.
pub(super) fn create_and_prefill_context<'model>(
    loaded: &'model LoadedModel,
    backend: &LlamaCppBackend,
    tokens: &[llama_cpp_2::token::LlamaToken],
    effective_ctx: usize,
    settings: &crate::model::ModelSettings,
) -> Result<llama_cpp_2::context::LlamaContext<'model>, ProviderError> {
    let ctx_params = build_context_params(effective_ctx as u32, settings);
    let mut ctx = loaded
        .model
        .new_context(backend.llama_backend(), ctx_params)
        .map_err(|e| ProviderError::ExecutionError(format!("Failed to create context: {}", e)))?;

    let n_batch = ctx.n_batch() as usize;
    for chunk in tokens.chunks(n_batch) {
        let mut batch = LlamaBatch::get_one(chunk)
            .map_err(|e| ProviderError::ExecutionError(format!("Failed to create batch: {}", e)))?;
        ctx.decode(&mut batch)
            .map_err(|e| ProviderError::ExecutionError(format!("Prefill decode failed: {}", e)))?;
    }

    Ok(ctx)
}

/// Tokenize text + images via mtmd and prefill the context.
///
/// Returns the llama context, the number of prompt tokens consumed,
/// and the effective context size.
pub(super) fn create_and_prefill_multimodal<'model>(
    loaded: &'model LoadedModel,
    backend: &LlamaCppBackend,
    prompt_text: &str,
    images: &[ExtractedImage],
    context_limit: usize,
    settings: &ModelSettings,
) -> Result<(llama_cpp_2::context::LlamaContext<'model>, usize, usize), ProviderError> {
    let mtmd_ctx = loaded.mtmd_ctx.as_ref().ok_or_else(|| {
        ProviderError::ExecutionError(
            "This model does not have vision support. Download the vision encoder from \
             Settings > Local Inference, or use a text-only message."
                .to_string(),
        )
    })?;

    let bitmaps: Vec<MtmdBitmap> = images
        .iter()
        .map(|img| {
            MtmdBitmap::from_buffer(mtmd_ctx, &img.bytes)
                .map_err(|e| ProviderError::ExecutionError(format!("Failed to decode image: {e}")))
        })
        .collect::<Result<_, _>>()?;

    let bitmap_refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();

    let input_text = MtmdInputText {
        text: prompt_text.to_string(),
        add_special: true,
        parse_special: true,
    };
    let chunks = mtmd_ctx.tokenize(input_text, &bitmap_refs).map_err(|e| {
        ProviderError::ExecutionError(format!("Multimodal tokenization failed: {e}"))
    })?;

    let prompt_token_count = chunks.total_tokens();

    let n_ctx_train = loaded.model.n_ctx_train() as usize;
    let mmproj_overhead = settings.mmproj_size_bytes;
    let memory_max_ctx = estimate_max_context_for_memory(&loaded.model, backend, mmproj_overhead);
    let effective_ctx = effective_context_size(
        prompt_token_count,
        settings,
        context_limit,
        n_ctx_train,
        memory_max_ctx,
    );

    let min_generation_headroom = 512;
    if prompt_token_count + min_generation_headroom > effective_ctx {
        return Err(ProviderError::ContextLengthExceeded(format!(
            "Multimodal prompt ({prompt_token_count} tokens including images) exceeds \
             context limit ({effective_ctx} tokens)",
        )));
    }

    let ctx_params = build_context_params(effective_ctx as u32, settings);
    let llama_ctx = loaded
        .model
        .new_context(backend.llama_backend(), ctx_params)
        .map_err(|e| ProviderError::ExecutionError(format!("Failed to create context: {e}")))?;

    let n_batch = llama_ctx.n_batch() as i32;
    let _n_past = chunks
        .eval_chunks(mtmd_ctx, &llama_ctx, 0, 0, n_batch, true)
        .map_err(|e| ProviderError::ExecutionError(format!("Multimodal eval failed: {e}")))?;

    Ok((llama_ctx, prompt_token_count, effective_ctx))
}

pub(super) fn prepare_generation<'model>(
    ctx: &mut GenerationContext<'model>,
    oai_messages_json: &str,
    full_tools_json: Option<&str>,
    compact_tools_json: Option<&str>,
) -> Result<PreparedGeneration<'model>, ProviderError> {
    let apply_template = |tools: Option<&str>| {
        let params = OpenAIChatTemplateParams {
            messages_json: oai_messages_json,
            tools_json: tools,
            tool_choice: None,
            json_schema: None,
            grammar: None,
            reasoning_format: if ctx.settings.enable_thinking {
                Some("auto")
            } else {
                None
            },
            chat_template_kwargs: None,
            add_generation_prompt: true,
            use_jinja: true,
            parallel_tool_calls: false,
            enable_thinking: ctx.settings.enable_thinking,
            add_bos: false,
            add_eos: false,
            parse_tool_calls: true,
        };
        ctx.loaded
            .model
            .apply_chat_template_oaicompat(ctx.template, &params)
    };

    let min_generation_headroom = 512;
    let n_ctx_train = ctx.loaded.model.n_ctx_train() as usize;
    let mmproj_overhead = if ctx.loaded.mtmd_ctx.is_some() {
        ctx.settings.mmproj_size_bytes
    } else {
        0
    };
    let memory_max_ctx =
        estimate_max_context_for_memory(&ctx.loaded.model, ctx.backend, mmproj_overhead);
    let cap = context_cap(ctx.settings, ctx.context_limit, n_ctx_train, memory_max_ctx);
    let token_budget = cap.saturating_sub(min_generation_headroom);
    let estimated_image_tokens = ctx.images.len() * ctx.settings.image_token_estimate;

    let template_result = match apply_template(full_tools_json) {
        Ok(r) => {
            let token_count = ctx
                .loaded
                .model
                .str_to_token(&r.prompt, AddBos::Never)
                .map(|t| t.len())
                .unwrap_or(0);
            if token_count + estimated_image_tokens > token_budget {
                apply_template(compact_tools_json).unwrap_or(r)
            } else {
                r
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Failed to apply llama.cpp OpenAI-compatible chat template"
            );
            match apply_template(compact_tools_json) {
                Ok(r) => r,
                Err(compact_err) => {
                    return Err(ProviderError::ExecutionError(format!(
                        "Failed to apply chat template with llama.cpp's Jinja renderer. This usually means the selected built-in template name does not exist, the embedded or custom template is invalid, or the template is incompatible with the current message shape. Select a valid llama.cpp built-in template name, configure a custom inline Jinja template, or use a GGUF with valid tokenizer.chat_template metadata. Full tools error: {e}; compact tools error: {compact_err}"
                    )));
                }
            }
        }
    };

    let _ = ctx.log.write(
        &serde_json::json!({"applied_prompt": &template_result.prompt}),
        None,
    );

    let (llama_ctx, prompt_token_count, effective_ctx) = if !ctx.images.is_empty() {
        create_and_prefill_multimodal(
            ctx.loaded,
            ctx.backend,
            &template_result.prompt,
            ctx.images,
            ctx.context_limit,
            ctx.settings,
        )?
    } else {
        let tokens = ctx
            .loaded
            .model
            .str_to_token(&template_result.prompt, AddBos::Never)
            .map_err(|e| ProviderError::ExecutionError(e.to_string()))?;
        let (ptc, ectx) = validate_and_compute_context(
            ctx.loaded,
            ctx.backend,
            tokens.len(),
            ctx.context_limit,
            ctx.settings,
        )?;
        let lctx =
            create_and_prefill_context(ctx.loaded, ctx.backend, &tokens, ectx, ctx.settings)?;
        (lctx, ptc, ectx)
    };

    Ok(PreparedGeneration {
        template_result,
        llama_ctx,
        prompt_token_count,
        effective_ctx,
    })
}

/// Action to take after processing a generated token piece.
pub(super) enum TokenAction {
    Continue,
    Stop,
}

/// Run the autoregressive generation loop. Calls `on_piece` for each non-empty
/// token piece. The callback returns `TokenAction::Stop` to break early.
/// Returns the total number of generated tokens, or `ContextLengthExceeded`
/// if the model exhausted the available context window.
pub(super) fn generation_loop(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    settings: &crate::model::ModelSettings,
    prompt_token_count: usize,
    effective_ctx: usize,
    mut on_piece: impl FnMut(&str) -> Result<TokenAction, ProviderError>,
) -> Result<i32, ProviderError> {
    let mut sampler = build_sampler(settings);
    let context_headroom = effective_ctx.saturating_sub(prompt_token_count);
    let max_output = if let Some(max) = settings.max_output_tokens {
        context_headroom.min(max)
    } else {
        context_headroom
    };
    let hit_context_limit = settings
        .max_output_tokens
        .is_none_or(|max| context_headroom <= max);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output_token_count: i32 = 0;
    let mut exhausted_loop = true;

    for _ in 0..max_output {
        let token = sampler.sample(ctx, -1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            exhausted_loop = false;
            break;
        }

        output_token_count += 1;

        let piece = model
            .token_to_piece(token, &mut decoder, true, None)
            .map_err(|e| ProviderError::ExecutionError(format!("Failed to decode token: {}", e)))?;

        if !piece.is_empty() && matches!(on_piece(&piece)?, TokenAction::Stop) {
            exhausted_loop = false;
            break;
        }

        let next_tokens = [token];
        let mut next_batch = LlamaBatch::get_one(&next_tokens)
            .map_err(|e| ProviderError::ExecutionError(format!("Failed to create batch: {}", e)))?;
        ctx.decode(&mut next_batch)
            .map_err(|e| ProviderError::ExecutionError(format!("Decode failed: {}", e)))?;
    }

    if exhausted_loop && hit_context_limit {
        return Err(ProviderError::ContextLengthExceeded(format!(
            "Generation exhausted context window ({} prompt + {} generated = {} of {} limit)",
            prompt_token_count,
            output_token_count,
            prompt_token_count as i32 + output_token_count,
            effective_ctx,
        )));
    }

    Ok(output_token_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelSettings;

    fn default_settings() -> ModelSettings {
        ModelSettings::default()
    }

    #[test]
    fn test_effective_context_size_uses_full_limit() {
        assert_eq!(
            effective_context_size(100, &default_settings(), 4096, 4096, None),
            4096
        );
    }

    #[test]
    fn test_effective_context_size_capped_by_limit() {
        assert_eq!(
            effective_context_size(100, &default_settings(), 1024, 8192, None),
            1024
        );
    }

    #[test]
    fn test_effective_context_size_capped_by_memory() {
        assert_eq!(
            effective_context_size(100, &default_settings(), 4096, 4096, Some(800)),
            800
        );
    }

    #[test]
    fn test_effective_context_size_memory_smaller_than_prompt() {
        assert_eq!(
            effective_context_size(600, &default_settings(), 4096, 4096, Some(700)),
            700
        );
    }

    #[test]
    fn test_effective_context_size_zero_limit_uses_train() {
        assert_eq!(
            effective_context_size(100, &default_settings(), 0, 2048, None),
            2048
        );
    }

    #[test]
    fn test_effective_context_size_prompt_exceeds_all_limits() {
        assert_eq!(
            effective_context_size(5000, &default_settings(), 4096, 4096, None),
            4096
        );
    }

    #[test]
    fn test_context_cap_with_settings_override() {
        let mut settings = default_settings();
        settings.context_size = Some(2048);
        assert_eq!(context_cap(&settings, 4096, 8192, Some(1024)), 2048);
    }

    #[test]
    fn test_context_cap_without_override() {
        assert_eq!(context_cap(&default_settings(), 4096, 8192, None), 4096);
    }

    #[test]
    fn test_context_cap_memory_limited() {
        assert_eq!(
            context_cap(&default_settings(), 4096, 8192, Some(2048)),
            2048
        );
    }
}
