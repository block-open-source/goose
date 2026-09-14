//! Local Whisper transcription using Candle
//!
//! This module provides local audio transcription using OpenAI's Whisper model
//! via the Candle ML framework. It supports loading GGUF quantized models for
//! efficient CPU inference.
//! Heavily "inspired" by the Candle Whisper example:
//! https://github.com/huggingface/candle/tree/main/candle-examples/whisper

use crate::config::paths::Paths;

pub const LOCAL_WHISPER_MODEL_CONFIG_KEY: &str = "LOCAL_WHISPER_MODEL";
pub const LOCAL_WHISPER_LANGUAGE_CONFIG_KEY: &str = "LOCAL_WHISPER_LANGUAGE";
const ENGLISH_LANGUAGE_TOKEN: u32 = 50259;
const LANGUAGE_TOKEN_COUNT: u32 = 99;
use anyhow::{Context, Result};
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::ops::log_softmax;
use candle_transformers::models::whisper::{self as m, audio, Config, N_FRAMES};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use symphonia::core::audio::conv::FromSample;
use symphonia::core::audio::sample::Sample;
use symphonia::core::audio::{Audio, AudioBuffer, GenericAudioBufferRef};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use tokenizers::Tokenizer;

// Common suppress tokens for all Whisper models
const SUPPRESS_TOKENS: &[u32] = &[
    1, 2, 7, 8, 9, 10, 14, 25, 26, 27, 28, 29, 31, 58, 59, 60, 61, 62, 63, 90, 91, 92, 93, 359,
    503, 522, 542, 873, 893, 902, 918, 922, 931, 1350, 1853, 1982, 2460, 2627, 3246, 3253, 3268,
    3536, 3846, 3961, 4183, 4667, 6585, 6647, 7273, 9061, 9383, 10428, 10929, 11938, 12033, 12331,
    12562, 13793, 14157, 14635, 15265, 15618, 16553, 16604, 18362, 18956, 20075, 21675, 22520,
    26130, 26161, 26435, 28279, 29464, 31650, 32302, 32470, 36865, 42863, 47425, 49870, 50254,
    50258, 50360, 50362,
];

// Special token IDs
const SOT_TOKEN: u32 = 50258;
const TRANSCRIBE_TOKEN: u32 = 50359;
const EOT_TOKEN: u32 = 50257;
const TIMESTAMP_BEGIN: u32 = 50364;
const SAMPLE_BEGIN: usize = 3;
const MIN_SUPPORTED_AUDIO_SAMPLE_RATE: u32 = 8_000;
const MAX_SUPPORTED_AUDIO_SAMPLE_RATE: u32 = 96_000;
const MAX_WHISPER_AUDIO_SAMPLES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperModel {
    /// Model identifier (e.g., "tiny", "base", "small")
    pub id: &'static str,
    /// Model file size in MB
    pub size_mb: u32,
    /// Download URL from HuggingFace
    pub url: &'static str,
    /// Description
    pub description: &'static str,
}

const MODELS: &[WhisperModel] = &[
    WhisperModel {
        id: "tiny",
        size_mb: 40,
        url: "https://huggingface.co/oxide-lab/whisper-tiny-GGUF/resolve/main/model-tiny-q80.gguf",
        description: "Fastest, ~2-3x realtime on CPU (5-10x with GPU)",
    },
    WhisperModel {
        id: "base",
        size_mb: 78,
        url: "https://huggingface.co/oxide-lab/whisper-base-GGUF/resolve/main/whisper-base-q8_0.gguf",
        description: "Good balance, ~1.5-2x realtime on CPU (4-8x with GPU)",
    },
    WhisperModel {
        id: "small",
        size_mb: 247,
        url: "https://huggingface.co/oxide-lab/whisper-small-GGUF/resolve/main/whisper-small-q8_0.gguf",
        description: "High accuracy, ~0.8-1x realtime on CPU (3-5x with GPU)",
    },
    WhisperModel {
        id: "medium",
        size_mb: 777,
        url: "https://huggingface.co/oxide-lab/whisper-medium-GGUF/resolve/main/whisper-medium-q8_0.gguf",
        description: "Highest accuracy, ~0.5x realtime on CPU (2-4x with GPU)",
    },
];

impl WhisperModel {
    pub fn local_path(&self) -> PathBuf {
        let filename = self.url.rsplit('/').next().unwrap_or("");
        Paths::in_data_dir("models").join(filename)
    }

    pub fn is_downloaded(&self) -> bool {
        self.local_path().exists()
    }

    pub fn config(&self) -> Config {
        match self.id {
            "tiny" => Config {
                num_mel_bins: 80,
                max_source_positions: 1500,
                d_model: 384,
                encoder_attention_heads: 6,
                encoder_layers: 4,
                decoder_attention_heads: 6,
                decoder_layers: 4,
                vocab_size: 51865,
                suppress_tokens: SUPPRESS_TOKENS.to_vec(),
                max_target_positions: 448,
            },
            "base" => Config {
                num_mel_bins: 80,
                max_source_positions: 1500,
                d_model: 512,
                encoder_attention_heads: 8,
                encoder_layers: 6,
                decoder_attention_heads: 8,
                decoder_layers: 6,
                vocab_size: 51865,
                suppress_tokens: SUPPRESS_TOKENS.to_vec(),
                max_target_positions: 448,
            },
            "small" => Config {
                num_mel_bins: 80,
                max_source_positions: 1500,
                d_model: 768,
                encoder_attention_heads: 12,
                encoder_layers: 12,
                decoder_attention_heads: 12,
                decoder_layers: 12,
                vocab_size: 51865,
                suppress_tokens: SUPPRESS_TOKENS.to_vec(),
                max_target_positions: 448,
            },
            "medium" => Config {
                num_mel_bins: 80,
                max_source_positions: 1500,
                d_model: 1024,
                encoder_attention_heads: 16,
                encoder_layers: 24,
                decoder_attention_heads: 16,
                decoder_layers: 24,
                vocab_size: 51865,
                suppress_tokens: SUPPRESS_TOKENS.to_vec(),
                max_target_positions: 448,
            },
            _ => {
                tracing::warn!("Unknown model '{}', falling back to tiny config", self.id);
                Config {
                    num_mel_bins: 80,
                    max_source_positions: 1500,
                    d_model: 384,
                    encoder_attention_heads: 6,
                    encoder_layers: 4,
                    decoder_attention_heads: 6,
                    decoder_layers: 4,
                    vocab_size: 51865,
                    suppress_tokens: SUPPRESS_TOKENS.to_vec(),
                    max_target_positions: 448,
                }
            }
        }
    }
}

pub fn available_models() -> &'static [WhisperModel] {
    MODELS
}

pub fn get_model(id: &str) -> Option<&'static WhisperModel> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn recommend_model() -> &'static str {
    let has_gpu_or_metal = Device::new_cuda(0).is_ok() || Device::new_metal(0).is_ok();

    if has_gpu_or_metal {
        "small"
    } else {
        let cpu_count = sys_info::cpu_num().unwrap_or(1) as u64;
        let cpu_speed_mhz = sys_info::cpu_speed().unwrap_or(0);

        if cpu_count * cpu_speed_mhz >= 16_000 {
            "base"
        } else {
            "tiny"
        }
    }
}

pub struct WhisperTranscriber {
    model: m::quantized_model::Whisper,
    config: Config,
    device: Device,
    mel_filters: Vec<f32>,
    tokenizer: Tokenizer,
    eot_token: u32,
    no_timestamps_token: u32,
    language_token: u32,
    max_initial_timestamp_index: u32,
}

impl WhisperTranscriber {
    pub fn new_with_tokenizer<P: AsRef<Path>>(
        model_id: &str,
        model_path: P,
        bundled_tokenizer: &str,
    ) -> Result<Self> {
        tracing::debug!(model_id, "initializing whisper transcriber");

        let device = if let Ok(device) = Device::new_cuda(0) {
            tracing::debug!("using CUDA device");
            device
        } else if let Ok(device) = Device::new_metal(0) {
            tracing::debug!("using Metal device");
            device
        } else {
            tracing::debug!("using CPU device");
            Device::Cpu
        };

        let model_path_ref = model_path.as_ref();
        tracing::debug!(path = %model_path_ref.display(), "loading model from path");

        if !model_path_ref.exists() {
            anyhow::bail!("Model file not found: {}", model_path_ref.display());
        }

        let model =
            get_model(model_id).ok_or_else(|| anyhow::anyhow!("Unknown model: {}", model_id))?;
        let config = model.config();
        tracing::debug!(
            num_mel_bins = config.num_mel_bins,
            d_model = config.d_model,
            "loaded model config"
        );

        let mel_bytes = match config.num_mel_bins {
            80 => include_bytes!("whisper_data/melfilters.bytes").as_slice(),
            128 => include_bytes!("whisper_data/melfilters128.bytes").as_slice(),
            nmel => anyhow::bail!("unexpected num_mel_bins {nmel}"),
        };
        let mut mel_filters = vec![0f32; mel_bytes.len() / 4];
        byteorder::ReadBytesExt::read_f32_into::<byteorder::LittleEndian>(
            &mut &mel_bytes[..],
            &mut mel_filters,
        )?;
        tracing::debug!(mel_filters_len = mel_filters.len(), "loaded mel filters");

        tracing::debug!("loading GGUF model weights");
        let vb = candle_transformers::quantized_var_builder::VarBuilder::from_gguf(
            model_path_ref,
            &device,
        )?;
        let model = m::quantized_model::Whisper::load(&vb, config.clone())?;
        tracing::debug!("model weights loaded successfully");

        tracing::debug!("loading tokenizer");
        let tokenizer = Self::load_tokenizer(model_path_ref, Some(bundled_tokenizer))?;
        tracing::debug!("tokenizer loaded successfully");

        let language_token = Self::resolve_language_token(&tokenizer);

        Ok(Self {
            model,
            config,
            device,
            mel_filters,
            tokenizer,
            eot_token: 50257,
            no_timestamps_token: 50363,
            language_token,
            max_initial_timestamp_index: 50,
        })
    }

    fn resolve_language_token(tokenizer: &Tokenizer) -> u32 {
        let configured = crate::config::Config::global()
            .get(LOCAL_WHISPER_LANGUAGE_CONFIG_KEY, false)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        language_token(tokenizer, configured.as_deref())
    }

    fn load_tokenizer(model_dir: &Path, bundled_tokenizer: Option<&str>) -> Result<Tokenizer> {
        let tokenizer_path = model_dir
            .parent()
            .unwrap_or(model_dir)
            .join("tokenizer.json");

        if tokenizer_path.exists() {
            return Tokenizer::from_file(tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e));
        }

        if let Some(tokenizer_json) = bundled_tokenizer {
            if let Some(parent) = tokenizer_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&tokenizer_path, tokenizer_json)?;
            return Tokenizer::from_file(tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e));
        }

        anyhow::bail!(
            "Tokenizer not found at {} and no bundled tokenizer provided",
            tokenizer_path.display()
        )
    }

    pub fn transcribe(&mut self, audio_data: &[u8]) -> Result<String> {
        tracing::debug!(audio_bytes = audio_data.len(), "starting transcription");

        if audio_data.is_empty() {
            tracing::debug!("empty audio data received");
            return Ok(String::new());
        }

        self.language_token = Self::resolve_language_token(&self.tokenizer);

        let (mel_tensor, actual_content_frames) = self.prepare_audio_input(audio_data)?;
        let (_, _, padded_frames) = mel_tensor.dims3()?;

        let content_frames = actual_content_frames.min(padded_frames);
        let audio_duration_secs = (content_frames * 160) as f32 / 16000.0;
        tracing::debug!(
            actual_content_frames,
            padded_frames,
            content_frames,
            audio_duration_secs,
            "prepared mel spectrogram"
        );

        if content_frames == 0 {
            tracing::debug!("no content frames in mel spectrogram");
            return Ok(String::new());
        }

        let num_segments = content_frames.div_ceil(N_FRAMES);
        tracing::debug!(
            num_segments,
            n_frames = N_FRAMES,
            "processing audio segments"
        );

        let mut all_text_tokens = Vec::new();
        let mut seek = 0;
        let mut segment_num = 0;

        while seek < content_frames {
            segment_num += 1;
            let segment_size = usize::min(content_frames - seek, N_FRAMES);
            tracing::debug!(segment_num, segment_size, seek, "processing segment");

            let segment_text_tokens =
                self.process_segment(&mel_tensor, seek, segment_size, segment_num, num_segments)?;

            tracing::debug!(
                tokens_in_segment = segment_text_tokens.len(),
                "segment produced tokens"
            );
            all_text_tokens.extend(segment_text_tokens);
            seek += segment_size;
        }

        tracing::debug!(
            total_tokens = all_text_tokens.len(),
            "decoding tokens to text"
        );

        if all_text_tokens.is_empty() {
            tracing::warn!(
                audio_bytes = audio_data.len(),
                audio_duration_secs,
                num_segments,
                "no tokens produced from audio - possible silence or unrecognized speech"
            );
            return Ok(String::new());
        }

        let raw_result = self.decode_tokens(&all_text_tokens)?;
        let result = deduplicate_text(&raw_result);
        if result != raw_result {
            tracing::debug!(
                before_len = raw_result.len(),
                after_len = result.len(),
                "text-level deduplication removed repeated phrases"
            );
        }
        tracing::debug!(result_len = result.len(), "transcription complete");
        Ok(result)
    }

    fn prepare_audio_input(&self, audio_data: &[u8]) -> Result<(Tensor, usize)> {
        tracing::debug!(audio_bytes = audio_data.len(), "decoding audio to PCM");
        let pcm_data = decode_audio_simple(audio_data)?;
        let pcm_samples = pcm_data.len();
        tracing::debug!(pcm_samples, "converting PCM to mel spectrogram");

        let actual_content_frames = pcm_samples / 160;

        let mel = audio::pcm_to_mel(&self.config, &pcm_data, &self.mel_filters);
        let mel_len = mel.len();
        tracing::debug!(
            mel_len,
            num_mel_bins = self.config.num_mel_bins,
            actual_content_frames,
            "creating mel tensor"
        );
        let mel_tensor = Tensor::from_vec(
            mel,
            (
                1,
                self.config.num_mel_bins,
                mel_len / self.config.num_mel_bins,
            ),
            &self.device,
        )?;

        Ok((mel_tensor, actual_content_frames))
    }

    fn process_segment(
        &mut self,
        mel_tensor: &Tensor,
        seek: usize,
        segment_size: usize,
        _segment_num: usize,
        _num_segments: usize,
    ) -> Result<Vec<u32>> {
        let _time_offset = (seek * 160) as f32 / 16000.0; // HOP_LENGTH = 160
        let _segment_duration = (segment_size * 160) as f32 / 16000.0;
        let mel_segment = mel_tensor.narrow(2, seek, segment_size)?;

        if tracing::enabled!(tracing::Level::DEBUG) {
            let mel_flat = mel_segment.flatten_all()?;
            let mel_mean: f32 = mel_flat.mean(0)?.to_scalar()?;
            let mel_max: f32 = mel_flat.max(0)?.to_scalar()?;
            let mel_min: f32 = mel_flat.min(0)?.to_scalar()?;
            tracing::debug!(mel_mean, mel_max, mel_min, "mel segment statistics");
        }

        self.model.decoder.reset_kv_cache();
        let audio_features = self.model.encoder.forward(&mel_segment, true)?;

        if tracing::enabled!(tracing::Level::DEBUG) {
            let af_flat = audio_features.flatten_all()?;
            let af_mean: f32 = af_flat.mean(0)?.to_scalar()?;
            let af_max: f32 = af_flat.max(0)?.to_scalar()?;
            let af_min: f32 = af_flat.min(0)?.to_scalar()?;
            tracing::debug!(af_mean, af_max, af_min, "audio features statistics");
        }
        let suppress_tokens = {
            let mut suppress = vec![0f32; self.config.vocab_size];
            for &token_id in &self.config.suppress_tokens {
                if (token_id as usize) < suppress.len() {
                    suppress[token_id as usize] = f32::NEG_INFINITY;
                }
            }
            suppress[self.no_timestamps_token as usize] = f32::NEG_INFINITY;
            Tensor::from_vec(suppress, self.config.vocab_size, &self.device)?
        };
        let mut tokens = vec![SOT_TOKEN, self.language_token, TRANSCRIBE_TOKEN];
        let sample_len = self.config.max_target_positions / 2;

        for i in 0..sample_len {
            let tokens_tensor = Tensor::new(tokens.as_slice(), &self.device)?.unsqueeze(0)?;
            let ys = self
                .model
                .decoder
                .forward(&tokens_tensor, &audio_features, i == 0)?;

            let (_, seq_len, _) = ys.dims3()?;
            let mut logits = self
                .model
                .decoder
                .final_linear(&ys.i((..1, seq_len - 1..))?)?
                .i(0)?
                .i(0)?;

            logits = self.apply_timestamp_rules(&logits, &tokens)?;
            let logits = logits.broadcast_add(&suppress_tokens)?;

            let logits_v: Vec<f32> = logits.to_vec1()?;
            let next_token = logits_v
                .iter()
                .enumerate()
                .max_by(|(_, u), (_, v)| u.total_cmp(v))
                .map(|(i, _)| i as u32)
                .unwrap();

            tokens.push(next_token);

            if next_token == EOT_TOKEN {
                tracing::debug!(tokens_generated = tokens.len() - 3, "EOT token received");
                break;
            }
            if tokens.len() > self.config.max_target_positions {
                tracing::debug!("max target positions reached");
                break;
            }

            if let Some(truncate_at) = self.detect_repetition(&tokens) {
                tracing::debug!(
                    truncate_at,
                    tokens_before = tokens.len(),
                    "repetition detected, truncating"
                );
                tokens.truncate(truncate_at);
                break;
            }
        }

        tracing::debug!(
            all_tokens = ?&tokens[3..],
            "all tokens generated in segment"
        );

        let segment_text_tokens: Vec<u32> = tokens[3..]
            .iter()
            .filter(|&&t| t != EOT_TOKEN && t < TIMESTAMP_BEGIN)
            .copied()
            .collect();

        if segment_text_tokens.is_empty() && tokens.len() > 3 {
            tracing::debug!(
                raw_tokens = ?&tokens[3..],
                "no text tokens found after filtering (all were EOT or timestamps)"
            );
        }

        Ok(segment_text_tokens)
    }

    fn apply_timestamp_rules(&self, input_logits: &Tensor, tokens: &[u32]) -> Result<Tensor> {
        let device = input_logits.device().clone();
        let vocab_size = self.model.config.vocab_size as u32;

        let sampled_tokens = if tokens.len() > SAMPLE_BEGIN {
            &tokens[SAMPLE_BEGIN..]
        } else {
            &[]
        };

        let mut masks = Vec::new();
        let mut mask_buffer = vec![0.0f32; vocab_size as usize];

        self.apply_timestamp_pairing_rule(
            sampled_tokens,
            vocab_size,
            &mut masks,
            &mut mask_buffer,
            &device,
        )?;
        self.apply_initial_timestamp_rule(
            tokens.len(),
            vocab_size,
            &mut masks,
            &mut mask_buffer,
            &device,
        )?;

        let mut logits = input_logits.clone();
        for mask in masks {
            logits = logits.broadcast_add(&mask)?;
        }

        logits =
            self.apply_timestamp_probability_rule(&logits, vocab_size, &mut mask_buffer, &device)?;

        Ok(logits)
    }

    fn apply_timestamp_pairing_rule(
        &self,
        sampled_tokens: &[u32],
        vocab_size: u32,
        masks: &mut Vec<Tensor>,
        mask_buffer: &mut [f32],
        device: &Device,
    ) -> Result<()> {
        if sampled_tokens.is_empty() {
            return Ok(());
        }

        let last_was_timestamp = sampled_tokens
            .last()
            .map(|&t| t >= TIMESTAMP_BEGIN)
            .unwrap_or(false);

        let penultimate_was_timestamp = if sampled_tokens.len() >= 2 {
            sampled_tokens[sampled_tokens.len() - 2] >= TIMESTAMP_BEGIN
        } else {
            true
        };

        if last_was_timestamp {
            if penultimate_was_timestamp {
                for i in 0..vocab_size {
                    mask_buffer[i as usize] = if i >= TIMESTAMP_BEGIN {
                        f32::NEG_INFINITY
                    } else {
                        0.0
                    };
                }
                masks.push(Tensor::new(mask_buffer as &[f32], device)?);
            } else {
                for i in 0..vocab_size {
                    mask_buffer[i as usize] = if i < self.eot_token {
                        f32::NEG_INFINITY
                    } else {
                        0.0
                    };
                }
                masks.push(Tensor::new(mask_buffer as &[f32], device)?);
            }
        }

        let timestamp_tokens: Vec<u32> = sampled_tokens
            .iter()
            .filter(|&&t| t >= TIMESTAMP_BEGIN)
            .cloned()
            .collect();

        if !timestamp_tokens.is_empty() {
            let timestamp_last = timestamp_tokens.last().unwrap() + 1;

            for i in 0..vocab_size {
                mask_buffer[i as usize] = if i >= TIMESTAMP_BEGIN && i < timestamp_last {
                    f32::NEG_INFINITY
                } else {
                    0.0
                };
            }
            masks.push(Tensor::new(mask_buffer as &[f32], device)?);
        }

        Ok(())
    }

    fn apply_initial_timestamp_rule(
        &self,
        tokens_len: usize,
        vocab_size: u32,
        masks: &mut Vec<Tensor>,
        mask_buffer: &mut [f32],
        device: &Device,
    ) -> Result<()> {
        if tokens_len != SAMPLE_BEGIN {
            return Ok(());
        }

        for i in 0..vocab_size {
            mask_buffer[i as usize] = if i < TIMESTAMP_BEGIN {
                f32::NEG_INFINITY
            } else {
                0.0
            };
        }
        masks.push(Tensor::new(mask_buffer as &[f32], device)?);

        let last_allowed = TIMESTAMP_BEGIN + self.max_initial_timestamp_index;
        if last_allowed < vocab_size {
            for i in 0..vocab_size {
                mask_buffer[i as usize] = if i > last_allowed {
                    f32::NEG_INFINITY
                } else {
                    0.0
                };
            }
            masks.push(Tensor::new(mask_buffer as &[f32], device)?);
        }

        Ok(())
    }

    fn apply_timestamp_probability_rule(
        &self,
        logits: &Tensor,
        vocab_size: u32,
        mask_buffer: &mut [f32],
        device: &Device,
    ) -> Result<Tensor> {
        let log_probs = log_softmax(logits, 0)?;

        let timestamp_log_probs = log_probs.narrow(
            0,
            TIMESTAMP_BEGIN as usize,
            vocab_size as usize - TIMESTAMP_BEGIN as usize,
        )?;

        let text_log_probs = log_probs.narrow(0, 0, TIMESTAMP_BEGIN as usize)?;

        let timestamp_logprob = {
            let max_val = timestamp_log_probs.max(0)?;
            let shifted = timestamp_log_probs.broadcast_sub(&max_val)?;
            let exp_shifted = shifted.exp()?;
            let sum_exp = exp_shifted.sum(0)?;
            let log_sum = sum_exp.log()?;
            max_val.broadcast_add(&log_sum)?.to_scalar::<f32>()?
        };

        let max_text_token_logprob: f32 = text_log_probs.max(0)?.to_scalar::<f32>()?;

        tracing::debug!(
            timestamp_logprob,
            max_text_token_logprob,
            "timestamp vs text probability comparison"
        );

        if timestamp_logprob > max_text_token_logprob {
            for i in 0..vocab_size {
                mask_buffer[i as usize] = if i < TIMESTAMP_BEGIN {
                    f32::NEG_INFINITY
                } else {
                    0.0
                };
            }
            let mask_tensor = Tensor::new(mask_buffer as &[f32], device)?;
            return logits.broadcast_add(&mask_tensor).map_err(Into::into);
        }

        Ok(logits.clone())
    }

    fn decode_tokens(&self, tokens: &[u32]) -> Result<String> {
        self.tokenizer
            .decode(tokens, true)
            .map_err(|e| anyhow::anyhow!("Failed to decode tokens: {}", e))
    }

    fn detect_repetition(&self, tokens: &[u32]) -> Option<usize> {
        detect_repetition_impl(tokens, SAMPLE_BEGIN, TIMESTAMP_BEGIN)
    }
}

/// Resolve an ISO 639-1 language code to its Whisper language token.
///
/// Whisper tokenizers expose one `<|xx|>` token per supported language, so the code is looked up
/// in the tokenizer rather than mapped through a table that would drift from the model.
/// Unset or unrecognized codes fall back to English.
///
/// The result is bounded to the language block because the tokenizer also contains task and
/// control tokens such as `<|translate|>`, which would otherwise resolve here and produce an
/// invalid decoder prompt.
fn language_token(tokenizer: &Tokenizer, code: Option<&str>) -> u32 {
    let Some(code) = code else {
        return ENGLISH_LANGUAGE_TOKEN;
    };

    let code = code.trim().to_lowercase();
    let languages = ENGLISH_LANGUAGE_TOKEN..ENGLISH_LANGUAGE_TOKEN + LANGUAGE_TOKEN_COUNT;

    match tokenizer.token_to_id(&format!("<|{}|>", code)) {
        Some(token) if languages.contains(&token) => token,
        _ => {
            tracing::warn!(
                language = %code,
                "unsupported LOCAL_WHISPER_LANGUAGE, transcribing as English"
            );
            ENGLISH_LANGUAGE_TOKEN
        }
    }
}

/// Remove repeated phrases from transcribed text.
///
/// Whisper models (especially smaller/quantized ones) tend to loop, producing output like
/// "I could build a record mode. I could build a record mode. I could build a record mode."
/// This function collapses adjacent duplicate sentences/phrases down to a single occurrence.
fn deduplicate_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Split into sentences on common boundaries (. ! ?)
    let sentences = split_into_sentences(trimmed);
    if sentences.len() <= 1 {
        return trimmed.to_string();
    }

    let mut result: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < sentences.len() {
        // Try to find a repeating pattern starting at position i.
        // Check pattern lengths from 1 sentence up to half the remaining sentences.
        let remaining = sentences.len() - i;
        let max_pattern_len = remaining / 2;
        let mut best_pattern_len = 0;
        let mut best_repeat_count = 0;
        let mut best_total_consumed = 0;

        for pattern_len in 1..=max_pattern_len {
            let pattern = &sentences[i..i + pattern_len];
            let mut count = 1;
            let mut pos = i + pattern_len;
            while pos + pattern_len <= sentences.len() {
                let candidate = &sentences[pos..pos + pattern_len];
                if pattern
                    .iter()
                    .zip(candidate.iter())
                    .all(|(a, b)| a.trim() == b.trim())
                {
                    count += 1;
                    pos += pattern_len;
                } else {
                    break;
                }
            }
            // Prefer the pattern that removes the most repeated sentences
            let total_consumed = pattern_len * count;
            if count >= 2 && total_consumed > best_total_consumed {
                best_pattern_len = pattern_len;
                best_repeat_count = count;
                best_total_consumed = total_consumed;
            }
        }

        if best_repeat_count >= 2 {
            // Keep only the first occurrence of the repeated pattern
            for j in 0..best_pattern_len {
                result.push(sentences[i + j]);
            }
            i += best_pattern_len * best_repeat_count;
        } else {
            result.push(sentences[i]);
            i += 1;
        }
    }

    result.join("").trim_end().to_string()
}

#[allow(clippy::string_slice)] // Splitting on ASCII punctuation; indices are always valid UTF-8 boundaries
fn split_into_sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut last = 0;
    let bytes = text.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'.' || b == b'!' || b == b'?' {
            // Include trailing whitespace with the sentence
            let mut end = i + 1;
            while end < bytes.len() && bytes[end] == b' ' {
                end += 1;
            }
            sentences.push(&text[last..end]);
            last = end;
        }
    }

    // Don't forget the trailing fragment (if any)
    if last < text.len() {
        sentences.push(&text[last..]);
    }

    sentences
}

/// Detect repetition in token sequence, returning the index to truncate to if repetition found.
/// Filters out timestamp tokens (>= timestamp_begin) when looking for patterns.
/// Returns Some(truncate_index) if repetition detected, None otherwise.
fn detect_repetition_impl(
    tokens: &[u32],
    sample_begin: usize,
    timestamp_begin: u32,
) -> Option<usize> {
    if tokens.len() <= sample_begin {
        return None;
    }

    // Filter out timestamp tokens to get just text tokens, but remember original positions
    let text_tokens: Vec<(usize, u32)> = tokens[sample_begin..]
        .iter()
        .enumerate()
        .filter(|(_, &t)| t < timestamp_begin)
        .map(|(i, &t)| (i + sample_begin, t))
        .collect();

    // Need at least 3 tokens to detect any repetition (e.g., [A, A, A])
    if text_tokens.len() < 3 {
        return None;
    }

    let n = text_tokens.len();

    // Try different pattern lengths, starting from 1
    for pattern_len in 1..=(n / 2) {
        // Check if the last `pattern_len` tokens match the preceding `pattern_len` tokens
        let pattern_start = n - pattern_len;
        let prev_pattern_start = n - 2 * pattern_len;

        let matches = (0..pattern_len)
            .all(|i| text_tokens[prev_pattern_start + i].1 == text_tokens[pattern_start + i].1);

        if !matches {
            continue;
        }

        // Found adjacent repeated pattern - count total repetitions
        let mut repeat_count = 2;
        let mut check_start = prev_pattern_start;

        while check_start >= pattern_len {
            let earlier_start = check_start - pattern_len;
            let still_matches = (0..pattern_len)
                .all(|i| text_tokens[earlier_start + i].1 == text_tokens[pattern_start + i].1);
            if still_matches {
                repeat_count += 1;
                check_start = earlier_start;
            } else {
                break;
            }
        }

        // Trigger on: 3+ repeats of anything, or 2 repeats of 5+ token patterns
        if repeat_count >= 3 || (repeat_count >= 2 && pattern_len >= 5) {
            // Return the original token position after the first pattern
            let first_pattern_end_text_idx = check_start + pattern_len;
            let truncate_pos = text_tokens[first_pattern_end_text_idx].0;
            return Some(truncate_pos);
        }
    }

    None
}

fn decode_audio_simple(audio_data: &[u8]) -> Result<Vec<f32>> {
    tracing::debug!(input_bytes = audio_data.len(), "decoding audio");
    let audio_vec = audio_data.to_vec();
    let cursor = Cursor::new(audio_vec);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let hint = Hint::new();

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("Failed to probe audio format - unsupported format")?;

    let track = format
        .default_track(TrackType::Audio)
        .context("No default audio track found")?;

    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .context("No audio codec parameters found")?
        .clone();

    let sample_rate = codec_params
        .sample_rate
        .context("No sample rate in audio track")?;
    checked_resampled_sample_count(0, sample_rate, 16_000)?;

    let channels = codec_params
        .channels
        .as_ref()
        .map(|channels| channels.count())
        .context("No channel information in audio track")?;

    tracing::debug!(sample_rate, channels, "audio format detected");

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .context("Failed to create audio decoder - please ensure browser sends WAV format audio")?;

    let mut mono_data = Vec::new();
    let mut packet_count = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e).context("Failed to read audio packet")?,
        };

        match decoder.decode(&packet) {
            Ok(decoded) => {
                checked_decoded_sample_count(mono_data.len(), decoded.frames(), sample_rate)?;
                mono_data.try_reserve(decoded.frames())?;
                append_audio_buffer_as_mono(&decoded, &mut mono_data);
                packet_count += 1;
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => {
                continue;
            }
            Err(e) => return Err(e).context("Failed to decode audio packet")?,
        }
    }

    tracing::debug!(
        packet_count,
        mono_samples = mono_data.len(),
        "decoded audio packets"
    );

    let resampled = if sample_rate != 16000 {
        tracing::debug!(from_rate = sample_rate, to_rate = 16000, "resampling audio");
        resample_audio(&mono_data, sample_rate, 16000)?
    } else {
        mono_data
    };

    if tracing::enabled!(tracing::Level::DEBUG) {
        if !resampled.is_empty() {
            let max_abs = resampled.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            let mean_abs = resampled.iter().map(|s| s.abs()).sum::<f32>() / resampled.len() as f32;
            let rms =
                (resampled.iter().map(|s| s * s).sum::<f32>() / resampled.len() as f32).sqrt();
            tracing::debug!(
                output_samples = resampled.len(),
                max_abs,
                mean_abs,
                rms,
                "audio decoding complete with PCM stats"
            );
        } else {
            tracing::debug!(output_samples = 0, "audio decoding complete (empty)");
        }
    }

    Ok(resampled)
}

fn append_audio_buffer_as_mono(buffer: &GenericAudioBufferRef<'_>, samples: &mut Vec<f32>) {
    match buffer {
        GenericAudioBufferRef::U8(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::U16(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::U24(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::U32(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::S8(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::S16(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::S24(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::S32(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::F32(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
        GenericAudioBufferRef::F64(buffer) => append_typed_audio_buffer_as_mono(buffer, samples),
    }
}

fn append_typed_audio_buffer_as_mono<S>(buffer: &AudioBuffer<S>, samples: &mut Vec<f32>)
where
    S: Sample,
    f32: FromSample<S>,
{
    let num_channels = buffer.num_planes();
    let channel_scale = 1.0 / num_channels as f32;

    for frame_idx in 0..buffer.frames() {
        let mut sum = 0.0;
        for ch_idx in 0..num_channels {
            if let Some(plane) = buffer.plane(ch_idx) {
                sum += f32::from_sample(plane[frame_idx]);
            }
        }
        samples.push(sum * channel_scale);
    }
}

fn resample_audio(data: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    use rubato::{
        audioadapter_buffers::direct::SequentialSliceOfVecs, Async, FixedAsync, Resampler,
        SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };

    let output_samples = checked_resampled_sample_count(data.len(), from_rate, to_rate)?;

    if from_rate == to_rate {
        return Ok(data.to_vec());
    }

    tracing::debug!(
        from_rate,
        to_rate,
        input_samples = data.len(),
        output_samples,
        "resampling audio"
    );

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: Some(0.95),
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let mut resampler = Async::<f32>::new_sinc(
        to_rate as f64 / from_rate as f64,
        2.0,
        &params,
        1024,
        1,
        FixedAsync::Input,
    )?;

    let waves_in = vec![data.to_vec()];
    let input = SequentialSliceOfVecs::new(&waves_in, 1, data.len())?;
    let output = resampler.process_all(&input, data.len(), None)?;
    let output = output.take_data();

    tracing::debug!(output_samples = output.len(), "resampling complete");
    Ok(output)
}

fn checked_resampled_sample_count(
    input_samples: usize,
    from_rate: u32,
    to_rate: u32,
) -> Result<usize> {
    anyhow::ensure!(
        (MIN_SUPPORTED_AUDIO_SAMPLE_RATE..=MAX_SUPPORTED_AUDIO_SAMPLE_RATE).contains(&from_rate),
        "Unsupported audio sample rate {from_rate} Hz; supported range is {MIN_SUPPORTED_AUDIO_SAMPLE_RATE}-{MAX_SUPPORTED_AUDIO_SAMPLE_RATE} Hz"
    );

    let from_rate = from_rate as usize;
    let to_rate = to_rate as usize;
    let whole_seconds = input_samples / from_rate;
    let partial_second_samples = input_samples % from_rate;
    let output_samples = whole_seconds
        .checked_mul(to_rate)
        .and_then(|whole| {
            partial_second_samples
                .checked_mul(to_rate)
                .and_then(|partial| whole.checked_add(partial.div_ceil(from_rate)))
        })
        .ok_or_else(|| anyhow::anyhow!("Resampled audio size overflow"))?;

    anyhow::ensure!(
        output_samples <= MAX_WHISPER_AUDIO_SAMPLES,
        "Whisper audio would contain {output_samples} samples; maximum is {MAX_WHISPER_AUDIO_SAMPLES}"
    );

    Ok(output_samples)
}

fn checked_decoded_sample_count(
    current_samples: usize,
    decoded_frames: usize,
    sample_rate: u32,
) -> Result<usize> {
    let decoded_samples = current_samples
        .checked_add(decoded_frames)
        .ok_or_else(|| anyhow::anyhow!("Decoded audio size overflow"))?;

    anyhow::ensure!(
        decoded_samples <= MAX_WHISPER_AUDIO_SAMPLES,
        "Decoded audio would contain {decoded_samples} samples; maximum is {MAX_WHISPER_AUDIO_SAMPLES}"
    );
    checked_resampled_sample_count(decoded_samples, sample_rate, 16_000)?;

    Ok(decoded_samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    use symphonia::core::audio::{AudioSpec, Position};
    use test_case::test_case;

    const TS: u32 = 50364; // A timestamp token for tests

    #[test]
    fn decodes_pcm_wav() {
        let input = [-32768i16, 0, 32767];
        let data_size = (input.len() * std::mem::size_of::<i16>()) as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        for sample in input {
            wav.extend_from_slice(&sample.to_le_bytes());
        }

        let decoded = decode_audio_simple(&wav).unwrap();

        assert_eq!(decoded.len(), input.len());
        assert_eq!(decoded[0], -1.0);
        assert_eq!(decoded[1], 0.0);
        assert!((decoded[2] - 32767.0 / 32768.0).abs() < f32::EPSILON);
    }

    #[test_case(None, ENGLISH_LANGUAGE_TOKEN ; "unset falls back to english")]
    #[test_case(Some("en"), ENGLISH_LANGUAGE_TOKEN ; "english")]
    #[test_case(Some("de"), 50261 ; "german")]
    #[test_case(Some("ru"), 50263 ; "russian")]
    #[test_case(Some("DE"), 50261 ; "uppercase code is normalized")]
    #[test_case(Some("klingon"), ENGLISH_LANGUAGE_TOKEN ; "unsupported falls back to english")]
    #[test_case(Some("su"), 50357 ; "last language in the block")]
    #[test_case(Some("translate"), ENGLISH_LANGUAGE_TOKEN ; "task token is not a language")]
    #[test_case(Some("notimestamps"), ENGLISH_LANGUAGE_TOKEN ; "control token is not a language")]
    #[test_case(Some("startoftranscript"), ENGLISH_LANGUAGE_TOKEN ; "sot token is not a language")]
    fn test_language_token(code: Option<&str>, expected: u32) {
        let tokenizer =
            Tokenizer::from_bytes(include_bytes!("whisper_data/tokens.json")).expect("tokenizer");

        assert_eq!(language_token(&tokenizer, code), expected);
    }

    // detect_repetition_impl tests
    // sample_begin=3 means tokens[0..3] are SOT, language, transcribe
    // timestamp_begin=50364 means tokens >= 50364 are timestamps

    #[test_case(&[0, 1, 2, 10, 10, 10], Some(4) ; "single token repeated 3x")]
    #[test_case(&[0, 1, 2, 10, 10], None ; "single token repeated 2x not enough")]
    #[test_case(&[0, 1, 2, 10, 20, 30, 10, 20, 30], None ; "3-token pattern repeated 2x not enough")]
    #[test_case(&[0, 1, 2, 10, 20, 30, 40, 50, 10, 20, 30, 40, 50], Some(8) ; "5-token pattern repeated 2x")]
    #[test_case(&[0, 1, 2, 10, 20, 10, 20, 10, 20], Some(5) ; "2-token pattern repeated 3x")]
    #[test_case(&[0, 1, 2, 10, 20, 30, 40, 10, 20, 30, 40], None ; "4-token pattern repeated 2x not enough")]
    #[test_case(&[0, 1, 2, 10, 99, 20, 10, 99, 20], None ; "non-adjacent same tokens no trigger")]
    fn test_detect_repetition_no_timestamps(tokens: &[u32], expected: Option<usize>) {
        assert_eq!(detect_repetition_impl(tokens, 3, 50364), expected);
    }

    #[test_case(
        &[0, 1, 2, TS, 10, 20, 30, TS+1, TS+2, 10, 20, 30, TS+3],
        None ;
        "phrase 3 tokens with timestamps 2x not enough"
    )]
    #[test_case(
        &[0, 1, 2, TS, 10, 10, 10, TS+1],
        Some(5) ;
        "single token 3x with surrounding timestamps"
    )]
    #[test_case(
        &[0, 1, 2, TS, 10, 20, TS+1, TS+2, 10, 20, TS+3, TS+4, 10, 20, TS+5],
        Some(8) ;
        "2-token pattern 3x with timestamps interleaved"
    )]
    fn test_detect_repetition_with_timestamps(tokens: &[u32], expected: Option<usize>) {
        assert_eq!(detect_repetition_impl(tokens, 3, 50364), expected);
    }

    // Real example from logs: phrase repeated 3x with timestamps
    #[test]
    fn test_detect_repetition_real_example() {
        let tokens: Vec<u32> = vec![
            0, 1, 2, // SOT, lang, transcribe (indices 0-2)
            50364, 286, 500, 380, 458, 983, 309, 311, 18617, 2564, 13, // first phrase + ts
            50450, 50475, 286, 500, 380, 458, 983, 309, 311, 18617, 2564,
            13, // second phrase + ts
            50550, 50551, 286, 500, 380, 458, 983, 309, 311, 18617, 2564,
            13, // third phrase + ts
        ];
        // Text tokens are: 286, 500, 380, 458, 983, 309, 311, 18617, 2564, 13 (10 tokens)
        // Repeated 3 times, should trigger
        let result = detect_repetition_impl(&tokens, 3, 50364);
        assert!(result.is_some(), "Should detect repetition in real example");
    }

    #[test]
    fn test_no_false_positive_on_dog_sentences() {
        // "I saw a dog. I liked the dog. I gave the dog food."
        // dog=100, other tokens are different
        let tokens: Vec<u32> = vec![
            0, 1, 2, 10, 11, 12, 100, 13, // I saw a dog.
            20, 21, 22, 100, 23, // I liked the dog.
            30, 31, 32, 100, 33, // I gave the dog food.
        ];
        assert_eq!(detect_repetition_impl(&tokens, 3, 50364), None);
    }

    // deduplicate_text tests
    #[test_case("", "" ; "empty")]
    #[test_case("   ", "" ; "whitespace")]
    #[test_case("I went to the store. Then I came home.", "I went to the store. Then I came home." ; "no repetition")]
    #[test_case(
        "I could build a record mode. I could build a record mode. I could build a record mode.",
        "I could build a record mode." ;
        "single sentence 3x"
    )]
    #[test_case(
        "Yeah I was thinking about that. Yeah I was thinking about that.",
        "Yeah I was thinking about that." ;
        "single sentence 2x"
    )]
    #[test_case(
        "Who works for Flux? Who works for Flux? Who works for Flux?",
        "Who works for Flux?" ;
        "question marks"
    )]
    #[test_case("Stop! Stop! Stop!", "Stop!" ; "exclamation marks")]
    #[test_case("hello hello hello hello", "hello hello hello hello" ; "no sentence boundaries")]
    fn test_deduplicate_text(input: &str, expected: &str) {
        assert_eq!(deduplicate_text(input), expected);
    }

    #[test_case("Hello. World. Foo.", vec!["Hello. ", "World. ", "Foo."] ; "basic")]
    #[test_case("Hello. World", vec!["Hello. ", "World"] ; "trailing fragment")]
    #[test_case("Really? Yes! Ok.", vec!["Really? ", "Yes! ", "Ok."] ; "mixed punctuation")]
    fn test_split_into_sentences(input: &str, expected: Vec<&str>) {
        assert_eq!(split_into_sentences(input), expected);
    }

    #[test_case(8_000, 48_000, 96_000 ; "minimum supported rate")]
    #[test_case(16_000, 48_000, 48_000 ; "whisper rate no-op")]
    #[test_case(96_000, 96_000, 16_000 ; "maximum supported rate")]
    fn supported_resample_rates_preserve_expected_size(
        from_rate: u32,
        input_samples: usize,
        expected_output_samples: usize,
    ) {
        assert_eq!(
            checked_resampled_sample_count(input_samples, from_rate, 16_000).unwrap(),
            expected_output_samples
        );
    }

    #[test]
    fn resampling_rejects_attacker_controlled_extreme_rate() {
        let error = resample_audio(&[0.0; 16], 1, 16_000).unwrap_err();

        assert!(error.to_string().contains("Unsupported audio sample rate"));
    }

    #[test]
    fn resampled_sample_budget_accepts_boundary_and_rejects_excess() {
        let boundary_input = MAX_WHISPER_AUDIO_SAMPLES / 2;
        assert_eq!(
            checked_resampled_sample_count(boundary_input, 8_000, 16_000).unwrap(),
            MAX_WHISPER_AUDIO_SAMPLES
        );

        let error = checked_resampled_sample_count(boundary_input + 1, 8_000, 16_000).unwrap_err();
        assert!(error.to_string().contains("maximum"));
    }

    #[test]
    fn decoded_audio_budget_enforces_native_whisper_rate() {
        assert_eq!(
            checked_decoded_sample_count(MAX_WHISPER_AUDIO_SAMPLES - 1, 1, 16_000).unwrap(),
            MAX_WHISPER_AUDIO_SAMPLES
        );

        let error =
            checked_decoded_sample_count(MAX_WHISPER_AUDIO_SAMPLES - 1, 2, 16_000).unwrap_err();
        assert!(error.to_string().contains("maximum"));
    }

    #[test]
    fn decoded_audio_budget_caps_high_rate_pcm() {
        let error = checked_decoded_sample_count(MAX_WHISPER_AUDIO_SAMPLES, 1, 96_000).unwrap_err();

        assert!(error.to_string().contains("maximum"));
    }

    #[test]
    fn decoded_stereo_frames_are_appended_as_mono() {
        let spec = AudioSpec::new(
            16_000,
            (Position::FRONT_LEFT | Position::FRONT_RIGHT).into(),
        );
        let mut buffer = AudioBuffer::<f32>::new(spec, 2);
        buffer
            .render_with(Some(2), |frame, planes| {
                planes[0][frame] = if frame == 0 { 1.0 } else { -1.0 };
                planes[1][frame] = if frame == 0 { 3.0 } else { 1.0 };
                Ok(())
            })
            .unwrap();
        let mut samples = vec![4.0];

        append_audio_buffer_as_mono(&GenericAudioBufferRef::F32(&buffer), &mut samples);

        assert_eq!(samples, vec![4.0, 2.0, 0.0]);
    }

    #[test]
    fn resampled_sample_count_rejects_overflow() {
        let error = checked_resampled_sample_count(usize::MAX, 8_000, 16_000).unwrap_err();

        assert!(error.to_string().contains("overflow"));
    }

    #[test]
    fn same_rate_resampling_preserves_samples() {
        let samples = vec![0.25, -0.5, 0.75];

        assert_eq!(resample_audio(&samples, 16_000, 16_000).unwrap(), samples);
    }
}
