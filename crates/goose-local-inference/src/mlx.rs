use std::collections::{HashMap, HashSet};
use std::path::Path;

use goose_provider_types::errors::ProviderError;

fn safetensors_shard(filename: &str) -> Option<(&str, u32, u32)> {
    let stem = filename.strip_suffix(".safetensors")?;
    let (indexed_name, total) = stem.rsplit_once("-of-")?;
    let (family, index) = indexed_name.rsplit_once('-')?;
    let index = index.parse().ok()?;
    let total = total.parse().ok()?;
    (index > 0 && index <= total).then_some((family, index, total))
}

pub(crate) fn snapshot_files_are_complete(
    filenames: &HashSet<&str>,
    index: Option<&serde_json::Value>,
) -> bool {
    let safetensors: Vec<_> = filenames
        .iter()
        .copied()
        .filter(|filename| filename.ends_with(".safetensors"))
        .collect();
    if safetensors.is_empty() {
        return false;
    }

    if let Some(index) = index {
        let Some(weight_map) = index.get("weight_map").and_then(|value| value.as_object()) else {
            return false;
        };
        let Some(expected): Option<HashSet<_>> =
            weight_map.values().map(|value| value.as_str()).collect()
        else {
            return false;
        };
        return !expected.is_empty()
            && expected.iter().all(|filename| {
                filename.ends_with(".safetensors") && filenames.contains(filename)
            });
    }

    let mut shard_groups = HashMap::new();
    for filename in safetensors {
        if let Some((family, index, total)) = safetensors_shard(filename) {
            let (expected_total, indices) = shard_groups
                .entry(family)
                .or_insert_with(|| (total, HashSet::new()));
            if *expected_total != total {
                return false;
            }
            indices.insert(index);
        }
    }

    shard_groups.values().all(|(total, indices)| {
        indices.len() == *total as usize && (1..=*total).all(|index| indices.contains(&index))
    })
}

fn validate_snapshot_files(path: &Path) -> Result<(), ProviderError> {
    let filenames: Vec<_> = std::fs::read_dir(path)
        .map_err(mlx_file_error)?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    let filename_set = filenames.iter().map(String::as_str).collect();
    let index_path = path.join("model.safetensors.index.json");
    let index = if index_path.is_file() {
        let contents = std::fs::read(&index_path).map_err(mlx_file_error)?;
        Some(serde_json::from_slice(&contents).map_err(mlx_file_error)?)
    } else {
        None
    };
    if !snapshot_files_are_complete(&filename_set, index.as_ref()) {
        return Err(ProviderError::ExecutionError(format!(
            "MLX model at '{}' has incomplete SafeTensors weights",
            path.display()
        )));
    }
    Ok(())
}

fn mlx_file_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::ExecutionError(format!("MLX model validation failed: {error}"))
}

#[cfg(all(feature = "mlx", target_os = "macos"))]
mod imp {
    use std::any::Any;
    use std::path::{Path, PathBuf};

    use safemlx::transforms::eval;
    use safemlx::{random, Array, Device, DeviceType, Stream};
    use safemlx_lm::gemma4_mtp::generate_gemma4_mtp;
    use safemlx_lm::models::{gemma4_assistant::load_gemma4_assistant_model, LoadedModel, Model};
    use safemlx_lm_utils::tokenizer::{Chat, Conversation, Role, Tokenizer};
    use serde_json::json;

    use crate::backend::{BackendLoadedModel, LocalGenerationRequest, LocalInferenceBackend};
    use crate::model::{ModelSettings, ToolCallingMode};
    use crate::native_tool_parsing::message_from_native_tool_text;
    use crate::provider_utils::filter_extensions_from_system_prompt;
    use crate::thinking_output::ThinkingOutputFilter;
    use crate::tool_emulation::{
        build_emulator_tool_description, load_tiny_model_prompt, message_for_emulator_action,
        StreamingEmulatorParser, CODE_EXECUTION_TOOL,
    };
    use crate::{extract_text_content, ResolvedModelPaths};
    use goose_provider_types::conversation::message::{Message, MessageContent};
    use goose_provider_types::conversation::token_usage::{
        DraftStats, ProviderStats, ProviderUsage, Usage,
    };
    use goose_provider_types::errors::ProviderError;
    use goose_provider_types::formats::openai;
    use goose_provider_types::images::ImageFormat;
    use goose_provider_types::request_log::LoggerHandleExt;

    pub(crate) const MLX_BACKEND_ID: &str = "mlx";

    pub(crate) struct MlxBackend;

    impl MlxBackend {
        pub(crate) fn new() -> Self {
            Self
        }
    }

    pub(crate) fn validate_model_directory(path: &Path) -> Result<(), ProviderError> {
        super::validate_snapshot_files(path)?;
        let config_path = path.join("config.json");
        let config = std::fs::read(&config_path).map_err(mlx_error)?;
        let config: serde_json::Value = serde_json::from_slice(&config).map_err(mlx_error)?;
        if let Some(reason) = safemlx_lm::check_model_config(&config).unsupported_reason() {
            return Err(ProviderError::ExecutionError(format!(
                "Unsupported MLX model at '{}': {}",
                path.display(),
                reason
            )));
        }
        Ok(())
    }

    impl LocalInferenceBackend for MlxBackend {
        fn id(&self) -> &'static str {
            MLX_BACKEND_ID
        }

        fn load_model(
            &self,
            model_id: &str,
            resolved: &ResolvedModelPaths,
            _settings: &ModelSettings,
        ) -> Result<Box<dyn BackendLoadedModel>, ProviderError> {
            if !resolved.model_path.exists() {
                return Err(ProviderError::ExecutionError(format!(
                    "Model not downloaded: {}. Please download it from Settings > Local Inference.",
                    model_id
                )));
            }

            let model_dir = model_dir_from_path(&resolved.model_path)?;
            let stream = Stream::new_with_device(&Device::new(DeviceType::Gpu, 0));
            let weights_stream = Stream::new_with_device(&Device::new(DeviceType::Cpu, 0));
            let model =
                LoadedModel::load(&model_dir, &stream, &weights_stream).map_err(mlx_error)?;
            let tokenizer =
                Tokenizer::from_file(model_dir.join("tokenizer.json")).map_err(mlx_error)?;
            tracing::info!(
                backend = self.id(),
                model_id,
                model_type = model.model_type(),
                "MLX model loaded successfully"
            );
            let stop_token_ids = mlx_stop_token_ids(&model, &model_dir);
            Ok(Box::new(MlxLoadedModel {
                model,
                tokenizer,
                model_dir,
                stop_token_ids,
            }))
        }

        fn generate(
            &self,
            loaded: &mut dyn BackendLoadedModel,
            request: LocalGenerationRequest<'_>,
        ) -> Result<(), ProviderError> {
            let loaded = loaded
                .as_any_mut()
                .downcast_mut::<MlxLoadedModel>()
                .ok_or_else(|| {
                    ProviderError::ExecutionError("Loaded model backend mismatch".to_string())
                })?;

            let stream = Stream::new_with_device(&Device::new(DeviceType::Gpu, 0));
            let tool_mode = if request.tools.is_empty() {
                ToolMode::None
            } else {
                match request.settings.tool_calling {
                    ToolCallingMode::ForceNative => ToolMode::Native,
                    ToolCallingMode::Auto | ToolCallingMode::ForceEmulated => ToolMode::Emulated {
                        code_mode_enabled: request
                            .tools
                            .iter()
                            .any(|t| t.name == CODE_EXECUTION_TOOL),
                    },
                }
            };
            let prompt = build_prompt(
                &mut loaded.model,
                &request.model_name,
                request.system,
                request.messages,
                request.tools,
                tool_mode,
            )?;
            let prompt_tokens = loaded.model.encode(&prompt, false).map_err(mlx_error)?;
            if prompt_tokens.len() >= request.context_limit && request.context_limit > 0 {
                return Err(ProviderError::ContextLengthExceeded(format!(
                    "Prompt ({} tokens) exceeds context limit ({} tokens). Try reducing conversation length.",
                    prompt_tokens.len(), request.context_limit
                )));
            }

            let prompt_array = loaded
                .model
                .encode_to_array(&prompt, false, &stream)
                .map_err(mlx_error)?;
            let max_tokens = mlx_max_tokens(
                request.settings,
                request.max_tokens,
                request.context_limit,
                prompt_tokens.len(),
            );
            let (settings_temp, seed) = sampling(request.settings);
            let temp = request.temperature.unwrap_or(settings_temp);
            let prng_key = prng_key(temp, seed)?;
            let eos_token_ids = loaded.stop_token_ids.clone();
            let generation_started = std::time::Instant::now();
            let MlxGeneration {
                generated_ids,
                generated_text,
                draft_stats,
                time_to_first_token_ms,
                streamed_response,
            } = if let Some(draft_model_path) = &request.draft_model_path {
                if matches!(loaded.model.model_mut(), Model::Gemma4(_)) {
                    let weights_stream = Stream::new_with_device(&Device::new(DeviceType::Cpu, 0));
                    let mut assistant =
                        load_gemma4_assistant_model(draft_model_path, &stream, &weights_stream)
                            .map_err(|error| {
                                mlx_error(format!("failed to load MLX draft model: {error}"))
                            })?;
                    let target = match loaded.model.model_mut() {
                        Model::Gemma4(target) => target,
                        _ => unreachable!(),
                    };
                    let (ids, stats) = generate_gemma4_mtp(
                        target,
                        &mut assistant,
                        &prompt_array,
                        &eos_token_ids,
                        max_tokens,
                        temp,
                        prng_key,
                        &stream,
                    )
                    .map_err(mlx_error)?;
                    let generated_text = loaded.tokenizer.decode(&ids, true).map_err(mlx_error)?;
                    MlxGeneration {
                        generated_ids: ids,
                        generated_text,
                        draft_stats: Some(DraftStats {
                            model: Some(draft_model_path.display().to_string()),
                            draft_tokens: stats.draft_tokens,
                            accepted_tokens: stats.accepted_tokens,
                            target_tokens: stats.target_tokens,
                            rounds: stats.rounds,
                            accept_rate: stats.accept_rate(),
                        }),
                        time_to_first_token_ms: None,
                        streamed_response: false,
                    }
                } else {
                    generate_single_model(
                        &mut loaded.model,
                        &loaded.tokenizer,
                        &prompt_array,
                        &eos_token_ids,
                        max_tokens,
                        temp,
                        prng_key,
                        &stream,
                        generation_started,
                        MlxStreamEmitter::new(
                            request.message_id,
                            tool_mode,
                            request.settings.enable_thinking,
                            &prompt,
                            request.tx,
                        ),
                    )?
                }
            } else {
                generate_single_model(
                    &mut loaded.model,
                    &loaded.tokenizer,
                    &prompt_array,
                    &eos_token_ids,
                    max_tokens,
                    temp,
                    prng_key,
                    &stream,
                    generation_started,
                    MlxStreamEmitter::new(
                        request.message_id,
                        tool_mode,
                        request.settings.enable_thinking,
                        &prompt,
                        request.tx,
                    ),
                )?
            };

            if !streamed_response {
                emit_generated_response(
                    &generated_text,
                    &prompt,
                    request.settings.enable_thinking,
                    request.message_id,
                    tool_mode,
                    request.tx,
                )?;
            }

            let output_tokens = generated_ids.len() as i32;
            let input_tokens = prompt_tokens.len() as i32;
            let usage = Usage::new(
                Some(input_tokens),
                Some(output_tokens),
                Some(input_tokens + output_tokens),
            );
            let log_json = serde_json::json!({
                "path": "mlx",
                "model_dir": loaded.model_dir,
                "prompt_tokens": input_tokens,
                "output_tokens": output_tokens,
                "model_load_ms": request.model_load_ms,
                "time_to_first_token_ms": time_to_first_token_ms,
                "elapsed_ms": generation_started.elapsed().as_millis() as u64,
                "generated_text": generated_text,
                "draft": draft_stats,
            });
            let _ = request.log.write(&log_json, Some(&usage));
            let stats = ProviderStats {
                time_to_first_token_ms,
                model_load_ms: request.model_load_ms,
                elapsed_ms: Some(generation_started.elapsed().as_millis() as u64),
                output_tokens: Some(generated_ids.len()),
                draft: draft_stats,
            };
            let provider_usage = ProviderUsage::new(request.model_name, usage).with_stats(stats);
            let _ = request.tx.blocking_send(Ok((None, Some(provider_usage))));
            Ok(())
        }

        fn available_memory_bytes(&self) -> u64 {
            0
        }
    }

    #[derive(Clone, Copy)]
    enum ToolMode {
        None,
        Native,
        Emulated { code_mode_enabled: bool },
    }

    struct MlxGeneration {
        generated_ids: Vec<u32>,
        generated_text: String,
        draft_stats: Option<DraftStats>,
        time_to_first_token_ms: Option<u64>,
        streamed_response: bool,
    }

    struct MlxLoadedModel {
        model: LoadedModel,
        tokenizer: Tokenizer,
        model_dir: PathBuf,
        stop_token_ids: Vec<u32>,
    }

    impl BackendLoadedModel for MlxLoadedModel {
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    fn model_dir_from_path(path: &Path) -> Result<PathBuf, ProviderError> {
        if path.is_dir() {
            Ok(path.to_path_buf())
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| mlx_error("MLX model path has no parent directory"))
        }
    }

    fn mlx_stop_token_ids(model: &LoadedModel, model_dir: &Path) -> Vec<u32> {
        let mut ids = model.eos_token_ids().to_vec();
        for id in generation_config_eos_token_ids(model_dir) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    fn generation_config_eos_token_ids(model_dir: &Path) -> Vec<u32> {
        let Ok(config_json) = std::fs::read_to_string(model_dir.join("generation_config.json"))
        else {
            return Vec::new();
        };
        let Ok(config) = serde_json::from_str::<serde_json::Value>(&config_json) else {
            return Vec::new();
        };
        match config.get("eos_token_id") {
            Some(value) => token_id_or_ids(value),
            None => Vec::new(),
        }
    }

    fn token_id_or_ids(value: &serde_json::Value) -> Vec<u32> {
        if let Some(id) = value.as_u64().and_then(|id| u32::try_from(id).ok()) {
            return vec![id];
        }
        value
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_u64().and_then(|id| u32::try_from(id).ok()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn build_prompt(
        model: &mut LoadedModel,
        model_name: &str,
        system: &str,
        messages: &[Message],
        tools: &[rmcp::model::Tool],
        tool_mode: ToolMode,
    ) -> Result<String, ProviderError> {
        match tool_mode {
            ToolMode::Native => {
                let conversations = openai_messages(system, messages);
                let tool_specs = openai::format_tools(tools)
                    .map_err(|e| ProviderError::ExecutionError(e.to_string()))?;
                if let Some(prompt) = model
                    .apply_chat_template_json([conversations], Some(&tool_specs), true)
                    .map_err(mlx_error)?
                {
                    return Ok(prompt);
                }

                Ok(render_prompt(system, messages))
            }
            ToolMode::Emulated { code_mode_enabled } => {
                let system_prompt = format!(
                    "{}{}",
                    load_tiny_model_prompt(),
                    build_emulator_tool_description(tools, code_mode_enabled)
                );
                if is_gemma4(model) {
                    let conversations = gemma4_messages_with_system(&system_prompt, messages);
                    if let Some(prompt) = model
                        .apply_chat_template_json([conversations], None, true)
                        .map_err(mlx_error)?
                    {
                        return Ok(prompt);
                    }
                }

                let conversations = chat_conversations(&system_prompt, messages);
                if let Some(prompt) = model
                    .apply_chat_template([Chat::Owned(conversations)], None, true)
                    .map_err(mlx_error)?
                {
                    return Ok(prompt);
                }

                Ok(render_prompt(&system_prompt, messages))
            }
            ToolMode::None => {
                if is_gemma4(model) {
                    let conversations = gemma4_messages(model_name, system, messages);
                    if let Some(prompt) = model
                        .apply_chat_template_json([conversations], None, true)
                        .map_err(mlx_error)?
                    {
                        return Ok(prompt);
                    }
                }

                let conversations = chat_conversations(system, messages);
                if let Some(prompt) = model
                    .apply_chat_template([Chat::Owned(conversations)], None, true)
                    .map_err(mlx_error)?
                {
                    return Ok(prompt);
                }

                Ok(render_prompt(system, messages))
            }
        }
    }

    fn generate_single_model(
        model: &mut LoadedModel,
        tokenizer: &Tokenizer,
        prompt_array: &Array,
        eos_token_ids: &[u32],
        max_tokens: usize,
        temp: f32,
        prng_key: Option<Array>,
        stream: &Stream,
        generation_started: std::time::Instant,
        mut emitter: MlxStreamEmitter<'_>,
    ) -> Result<MlxGeneration, ProviderError> {
        let mut cache = model.new_cache();
        let mut generated_ids = Vec::new();
        let mut streamed_text = String::new();
        let mut time_to_first_token_ms = None;
        let stream_generation = emitter.can_stream();
        let mut decode_stream = tokenizer.decode_stream(true);
        {
            let generator = model
                .generate_with_cache(&mut cache, temp, prompt_array, prng_key, stream)
                .take(max_tokens);
            for token in generator {
                let token = token.map_err(mlx_error)?;
                eval([&token]).map_err(mlx_error)?;
                let token_id = token.item::<u32>(stream);
                time_to_first_token_ms.get_or_insert_with(|| {
                    u64::try_from(generation_started.elapsed().as_millis()).unwrap_or(u64::MAX)
                });
                if eos_token_ids.contains(&token_id) {
                    break;
                }
                generated_ids.push(token_id);
                if stream_generation {
                    if let Some(piece) = decode_stream.step(token_id).map_err(mlx_error)? {
                        if !piece.is_empty() {
                            let should_continue = emitter.push_text(&piece)?;
                            streamed_text.push_str(&piece);
                            if !should_continue {
                                break;
                            }
                        }
                    }
                }
            }
        }
        let generated_text = tokenizer.decode(&generated_ids, true).map_err(mlx_error)?;
        let streamed_response = if stream_generation {
            match final_stream_suffix(&generated_text, &streamed_text)? {
                Some(suffix) => {
                    if !suffix.is_empty() {
                        emitter.push_text(suffix)?;
                    }
                    true
                }
                None => false,
            }
        } else {
            false
        };
        if streamed_response {
            emitter.finish()?;
        }
        Ok(MlxGeneration {
            generated_ids,
            generated_text,
            draft_stats: None,
            time_to_first_token_ms,
            streamed_response,
        })
    }

    fn final_stream_suffix<'a>(
        generated_text: &'a str,
        streamed_text: &str,
    ) -> Result<Option<&'a str>, ProviderError> {
        if streamed_text.is_empty() {
            return Ok(None);
        }

        generated_text
            .strip_prefix(streamed_text)
            .map(Some)
            .ok_or_else(|| mlx_error("streamed MLX decode did not match final tokenizer decode"))
    }

    fn mlx_max_tokens(
        settings: &ModelSettings,
        request_max_tokens: Option<i32>,
        context_limit: usize,
        prompt_tokens: usize,
    ) -> usize {
        let configured_max = settings
            .max_output_tokens
            .or_else(|| request_max_tokens.and_then(|tokens| usize::try_from(tokens).ok()));
        if context_limit == 0 {
            return configured_max.unwrap_or(4096);
        }

        let context_headroom = context_limit.saturating_sub(prompt_tokens);
        configured_max
            .map(|max| max.min(context_headroom))
            .unwrap_or(context_headroom)
    }

    fn is_gemma4(model: &LoadedModel) -> bool {
        matches!(model.model_type(), "gemma4" | "gemma4_text")
    }

    fn gemma4_messages(
        model_name: &str,
        system: &str,
        messages: &[Message],
    ) -> Vec<serde_json::Value> {
        let system = gemma4_system_prompt(model_name, system);
        gemma4_messages_with_optional_system(system.as_deref(), messages)
    }

    fn gemma4_messages_with_system(system: &str, messages: &[Message]) -> Vec<serde_json::Value> {
        gemma4_messages_with_optional_system(Some(system), messages)
    }

    fn gemma4_messages_with_optional_system(
        system: Option<&str>,
        messages: &[Message],
    ) -> Vec<serde_json::Value> {
        let mut values = Vec::new();
        if let Some(system) = system.map(str::trim).filter(|system| !system.is_empty()) {
            values.push(json!({
                "role": "system",
                "content": system,
            }));
        }

        for message in messages.iter().filter(|message| message.is_agent_visible()) {
            let text = extract_text_content(message);
            if text.trim().is_empty() {
                continue;
            }

            match message.role {
                rmcp::model::Role::User => values.push(json!({
                    "role": "user",
                    "content": [{"type": "text", "text": text.trim(), "content": text.trim()}],
                })),
                rmcp::model::Role::Assistant => values.push(json!({
                    "role": "assistant",
                    "content": text.trim(),
                })),
            }
        }

        values
    }

    fn gemma4_system_prompt(model_name: &str, system: &str) -> Option<String> {
        if should_use_tiny_system_prompt(model_name) {
            return Some(load_tiny_model_prompt());
        }

        let filtered = filter_extensions_from_system_prompt(system);
        let system = filtered.trim();
        if system.is_empty() {
            None
        } else {
            Some(system.to_string())
        }
    }

    fn should_use_tiny_system_prompt(model_name: &str) -> bool {
        estimate_model_size_billions(model_name).is_some_and(|size| size <= 4.0)
    }

    fn estimate_model_size_billions(model_name: &str) -> Option<f32> {
        let normalized = model_name.to_ascii_lowercase().replace('-', "_");
        for part in normalized.split('_') {
            if let Some(value) = part.strip_suffix('b') {
                if let Ok(size) = value.parse::<f32>() {
                    return Some(size);
                }
            }
            if let Some(value) = part
                .strip_prefix('e')
                .and_then(|value| value.strip_suffix('b'))
            {
                if let Ok(size) = value.parse::<f32>() {
                    return Some(size);
                }
            }
        }
        None
    }

    fn openai_messages(system: &str, messages: &[Message]) -> Vec<serde_json::Value> {
        let mut values = vec![serde_json::json!({
            "role": "system",
            "content": system,
        })];
        values.extend(openai::format_messages(messages, &ImageFormat::OpenAi));
        values
    }

    fn chat_conversations(system: &str, messages: &[Message]) -> Vec<Conversation<Role, String>> {
        let mut conversations = Vec::new();
        if !system.trim().is_empty() {
            conversations.push(Conversation {
                role: Role::System,
                content: system.trim().to_string(),
            });
        }
        for message in messages.iter().filter(|message| message.is_agent_visible()) {
            let role = match message.role {
                rmcp::model::Role::User => Role::User,
                rmcp::model::Role::Assistant => Role::Assistant,
            };
            let text = extract_text_content(message);
            if !text.trim().is_empty() {
                conversations.push(Conversation {
                    role,
                    content: text.trim().to_string(),
                });
            }
        }
        conversations
    }

    fn emit_generated_response(
        generated_text: &str,
        generation_prompt: &str,
        enable_thinking: bool,
        message_id: &str,
        tool_mode: ToolMode,
        tx: &tokio::sync::mpsc::Sender<
            Result<(Option<Message>, Option<ProviderUsage>), ProviderError>,
        >,
    ) -> Result<(), ProviderError> {
        if generated_text.is_empty() {
            return Ok(());
        }

        let (content, thinking) =
            split_generated_thinking(generated_text, generation_prompt, enable_thinking);

        match tool_mode {
            ToolMode::None => {
                emit_assistant_message(message_id, &thinking, &content, tx)?;
            }
            ToolMode::Native => {
                if let Some(mut message) = message_from_native_tool_text(&content, message_id)? {
                    prepend_thinking(&mut message, &thinking);
                    tx.blocking_send(Ok((Some(message), None))).map_err(|_| {
                        ProviderError::ExecutionError("Failed to stream MLX response".to_string())
                    })?;
                } else {
                    emit_assistant_message(message_id, &thinking, &content, tx)?;
                }
            }
            ToolMode::Emulated { code_mode_enabled } => {
                emit_assistant_message(message_id, &thinking, "", tx)?;
                let mut parser = StreamingEmulatorParser::new(code_mode_enabled);
                let mut actions = parser.process_chunk(&content);
                actions.extend(parser.flush());

                for action in actions {
                    let (message, _) = message_for_emulator_action(&action, message_id);
                    tx.blocking_send(Ok((Some(message), None))).map_err(|_| {
                        ProviderError::ExecutionError("Failed to stream MLX response".to_string())
                    })?;
                }
            }
        }
        Ok(())
    }

    struct MlxStreamEmitter<'a> {
        message_id: &'a str,
        tool_mode: ToolMode,
        tx: &'a tokio::sync::mpsc::Sender<
            Result<(Option<Message>, Option<ProviderUsage>), ProviderError>,
        >,
        output_filter: ThinkingOutputFilter,
        emulator_parser: Option<StreamingEmulatorParser>,
        stop_after_tool_call: bool,
    }

    impl<'a> MlxStreamEmitter<'a> {
        fn new(
            message_id: &'a str,
            tool_mode: ToolMode,
            enable_thinking: bool,
            generation_prompt: &str,
            tx: &'a tokio::sync::mpsc::Sender<
                Result<(Option<Message>, Option<ProviderUsage>), ProviderError>,
            >,
        ) -> Self {
            let emulator_parser = match tool_mode {
                ToolMode::Emulated { code_mode_enabled } => {
                    Some(StreamingEmulatorParser::new(code_mode_enabled))
                }
                ToolMode::None | ToolMode::Native => None,
            };
            Self {
                message_id,
                tool_mode,
                tx,
                output_filter: ThinkingOutputFilter::new(enable_thinking, generation_prompt),
                emulator_parser,
                stop_after_tool_call: false,
            }
        }

        fn can_stream(&self) -> bool {
            !matches!(self.tool_mode, ToolMode::Native)
        }

        fn push_text(&mut self, text: &str) -> Result<bool, ProviderError> {
            let filtered = self.output_filter.push_text(text);
            if !filtered.content.is_empty() {
                self.emit_content(&filtered.content)?;
            }
            Ok(!self.stop_after_tool_call)
        }

        fn finish(&mut self) -> Result<(), ProviderError> {
            self.flush_filtered_output()?;
            let actions = self
                .emulator_parser
                .as_mut()
                .map(StreamingEmulatorParser::flush)
                .unwrap_or_default();
            for action in actions {
                let (message, is_tool) = message_for_emulator_action(&action, self.message_id);
                if is_tool {
                    self.flush_filtered_output()?;
                }
                self.send(message)?;
                self.stop_after_tool_call |= is_tool;
                if is_tool {
                    break;
                }
            }
            Ok(())
        }

        fn flush_filtered_output(&mut self) -> Result<(), ProviderError> {
            let filtered = self.output_filter.finish();
            if !filtered.thinking.is_empty() {
                let mut message = Message::assistant().with_thinking(filtered.thinking, "");
                message.id = Some(self.message_id.to_string());
                self.send(message)?;
            }
            if !filtered.content.is_empty() {
                self.emit_content(&filtered.content)?;
            }
            Ok(())
        }

        fn emit_content(&mut self, content: &str) -> Result<(), ProviderError> {
            match self.tool_mode {
                ToolMode::None => {
                    let mut message = Message::assistant().with_text(content);
                    message.id = Some(self.message_id.to_string());
                    self.send(message)
                }
                ToolMode::Emulated { .. } => {
                    let actions = self
                        .emulator_parser
                        .as_mut()
                        .map(|parser| parser.process_chunk(content))
                        .unwrap_or_default();
                    for action in actions {
                        let (message, is_tool) =
                            message_for_emulator_action(&action, self.message_id);
                        if is_tool {
                            self.flush_filtered_output()?;
                        }
                        self.send(message)?;
                        self.stop_after_tool_call |= is_tool;
                        if is_tool {
                            break;
                        }
                    }
                    Ok(())
                }
                ToolMode::Native => Ok(()),
            }
        }

        fn send(&self, message: Message) -> Result<(), ProviderError> {
            self.tx
                .blocking_send(Ok((Some(message), None)))
                .map_err(|_| {
                    ProviderError::ExecutionError("Failed to stream MLX response".to_string())
                })
        }
    }

    fn split_generated_thinking(
        generated_text: &str,
        generation_prompt: &str,
        enable_thinking: bool,
    ) -> (String, String) {
        let mut filter = ThinkingOutputFilter::new(enable_thinking, generation_prompt);
        let mut filtered = filter.push_text(generated_text);
        let final_filtered = filter.finish();
        filtered.content.push_str(&final_filtered.content);
        filtered.thinking.push_str(&final_filtered.thinking);
        (filtered.content, filtered.thinking)
    }

    fn emit_assistant_message(
        message_id: &str,
        thinking: &str,
        content: &str,
        tx: &tokio::sync::mpsc::Sender<
            Result<(Option<Message>, Option<ProviderUsage>), ProviderError>,
        >,
    ) -> Result<(), ProviderError> {
        if thinking.is_empty() && content.is_empty() {
            return Ok(());
        }

        let mut message = Message::assistant();
        if !thinking.is_empty() {
            message = message.with_thinking(thinking, "");
        }
        if !content.is_empty() {
            message = message.with_text(content);
        }
        message.id = Some(message_id.to_string());
        tx.blocking_send(Ok((Some(message), None)))
            .map_err(|_| ProviderError::ExecutionError("Failed to stream MLX response".to_string()))
    }

    fn prepend_thinking(message: &mut Message, thinking: &str) {
        if !thinking.is_empty() {
            message
                .content
                .insert(0, MessageContent::thinking(thinking, ""));
        }
    }

    fn sampling(settings: &ModelSettings) -> (f32, Option<u32>) {
        match &settings.sampling {
            crate::model::SamplingConfig::Greedy => (0.0, None),
            crate::model::SamplingConfig::Temperature {
                temperature, seed, ..
            } => (*temperature, *seed),
            crate::model::SamplingConfig::MirostatV2 { seed, .. } => (0.0, *seed),
        }
    }

    fn prng_key(temp: f32, seed: Option<u32>) -> Result<Option<Array>, ProviderError> {
        if temp == 0.0 {
            return Ok(None);
        }
        random::key(seed.unwrap_or(0) as u64)
            .map(Some)
            .map_err(mlx_error)
    }

    fn render_prompt(system: &str, messages: &[Message]) -> String {
        let mut prompt = String::new();
        if !system.trim().is_empty() {
            prompt.push_str("System: ");
            prompt.push_str(system.trim());
            prompt.push('\n');
        }
        for message in messages.iter().filter(|message| message.is_agent_visible()) {
            let role = match message.role {
                rmcp::model::Role::User => "User",
                rmcp::model::Role::Assistant => "Assistant",
            };
            let text = extract_text_content(message);
            if !text.trim().is_empty() {
                prompt.push_str(role);
                prompt.push_str(": ");
                prompt.push_str(text.trim());
                prompt.push('\n');
            }
        }
        prompt.push_str("Assistant: ");
        prompt
    }

    fn mlx_error(error: impl std::fmt::Display) -> ProviderError {
        ProviderError::ExecutionError(format!("MLX backend error: {}", error))
    }

    #[cfg(test)]
    mod tests {
        use super::{
            final_stream_suffix, mlx_max_tokens, split_generated_thinking, token_id_or_ids,
        };
        use crate::model::ModelSettings;
        use serde_json::json;

        #[test]
        fn extracts_thinking_started_by_generation_prompt() {
            let (content, thinking) = split_generated_thinking(
                "hidden reasoning</think>visible answer",
                "<|im_start|>assistant\n<think>\n",
                true,
            );

            assert_eq!(thinking.trim(), "hidden reasoning");
            assert_eq!(content, "visible answer");
        }

        #[test]
        fn leaves_think_tags_as_content_when_thinking_disabled() {
            let generated = "hidden reasoning</think>visible answer";
            let (content, thinking) =
                split_generated_thinking(generated, "<|im_start|>assistant\n<think>\n", false);

            assert!(thinking.is_empty());
            assert_eq!(content, generated);
        }

        #[test]
        fn parses_single_and_multiple_eos_token_ids() {
            assert_eq!(token_id_or_ids(&json!(248044)), vec![248044]);
            assert_eq!(
                token_id_or_ids(&json!([248046, 248044])),
                vec![248046, 248044]
            );
        }

        #[test]
        fn final_stream_suffix_flushes_append_only_suffix() {
            assert_eq!(
                final_stream_suffix("hello world", "hello").unwrap(),
                Some(" world")
            );
        }

        #[test]
        fn final_stream_suffix_does_not_replay_fully_streamed_text() {
            assert_eq!(
                final_stream_suffix("run tool", "run tool").unwrap(),
                Some("")
            );
        }

        #[test]
        fn final_stream_suffix_allows_unstreamed_fallback() {
            assert_eq!(final_stream_suffix("hello world", "").unwrap(), None);
        }

        #[test]
        fn final_stream_suffix_rejects_rewritten_streamed_prefix() {
            assert!(final_stream_suffix("corrected response", "stale prefix").is_err());
        }

        #[test]
        fn max_tokens_defaults_to_context_headroom() {
            let settings = ModelSettings::default();

            assert_eq!(mlx_max_tokens(&settings, None, 128_000, 1_752), 126_248);
        }

        #[test]
        fn max_tokens_respects_configured_caps() {
            let mut settings = ModelSettings::default();
            settings.max_output_tokens = Some(2048);

            assert_eq!(mlx_max_tokens(&settings, None, 128_000, 1_752), 2048);

            let settings = ModelSettings::default();
            assert_eq!(mlx_max_tokens(&settings, Some(1024), 128_000, 1_752), 1024);
        }
    }
}

#[cfg(not(all(feature = "mlx", target_os = "macos")))]
mod imp {
    use std::path::Path;

    use crate::backend::{BackendLoadedModel, LocalGenerationRequest, LocalInferenceBackend};
    use crate::model::ModelSettings;
    use crate::ResolvedModelPaths;
    use goose_provider_types::errors::ProviderError;

    pub(crate) const MLX_BACKEND_ID: &str = "mlx";

    pub(crate) struct MlxBackend;

    impl MlxBackend {
        pub(crate) fn new() -> Self {
            Self
        }
    }

    pub(crate) fn validate_model_directory(path: &Path) -> Result<(), ProviderError> {
        super::validate_snapshot_files(path)?;
        let reason = if cfg!(target_os = "macos") {
            "MLX support was not compiled in. Rebuild with the `mlx` feature."
        } else {
            "MLX backend requires macOS."
        };
        Err(ProviderError::ExecutionError(reason.to_string()))
    }

    impl LocalInferenceBackend for MlxBackend {
        fn id(&self) -> &'static str {
            MLX_BACKEND_ID
        }

        fn load_model(
            &self,
            _model_id: &str,
            _resolved: &ResolvedModelPaths,
            _settings: &ModelSettings,
        ) -> Result<Box<dyn BackendLoadedModel>, ProviderError> {
            Err(ProviderError::ExecutionError(
                "MLX backend support was not compiled in. Rebuild with the `mlx` feature."
                    .to_string(),
            ))
        }

        fn generate(
            &self,
            _loaded: &mut dyn BackendLoadedModel,
            _request: LocalGenerationRequest<'_>,
        ) -> Result<(), ProviderError> {
            Err(ProviderError::ExecutionError(
                "MLX backend support was not compiled in. Rebuild with the `mlx` feature."
                    .to_string(),
            ))
        }

        fn available_memory_bytes(&self) -> u64 {
            0
        }
    }
}

pub(crate) use imp::{validate_model_directory, MlxBackend, MLX_BACKEND_ID};
