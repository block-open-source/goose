use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SamplingConfig {
    Greedy,
    Temperature {
        temperature: f32,
        top_k: i32,
        top_p: f32,
        min_p: f32,
        seed: Option<u32>,
    },
    MirostatV2 {
        tau: f32,
        eta: f32,
        seed: Option<u32>,
    },
}

impl Default for SamplingConfig {
    fn default() -> Self {
        SamplingConfig::Temperature {
            temperature: 0.8,
            top_k: 40,
            top_p: 0.95,
            min_p: 0.05,
            seed: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallingMode {
    #[default]
    Auto,
    ForceNative,
    ForceEmulated,
}

#[derive(Debug, Clone, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatTemplate {
    #[serde(alias = "auto")]
    #[default]
    Embedded,
    Builtin {
        name: String,
    },
    CustomInline {
        template: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
    pub context_size: Option<u32>,
    pub max_output_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_model: Option<String>,
    #[serde(default)]
    pub sampling: SamplingConfig,
    #[serde(default = "default_repeat_penalty")]
    pub repeat_penalty: f32,
    #[serde(default = "default_repeat_last_n")]
    pub repeat_last_n: i32,
    #[serde(default)]
    pub frequency_penalty: f32,
    #[serde(default)]
    pub presence_penalty: f32,
    pub n_batch: Option<u32>,
    pub n_gpu_layers: Option<u32>,
    #[serde(default)]
    pub use_mlock: bool,
    pub flash_attention: Option<bool>,
    pub n_threads: Option<i32>,
    #[serde(default)]
    pub tool_calling: ToolCallingMode,
    #[serde(default)]
    pub chat_template: ChatTemplate,
    #[serde(default = "default_true")]
    pub enable_thinking: bool,
    #[serde(default)]
    pub vision_capable: bool,
    #[serde(default = "default_image_token_estimate")]
    pub image_token_estimate: usize,
    #[serde(default)]
    pub mmproj_size_bytes: u64,
}

fn default_true() -> bool {
    true
}

fn default_image_token_estimate() -> usize {
    256
}

fn default_repeat_penalty() -> f32 {
    1.0
}

fn default_repeat_last_n() -> i32 {
    64
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            backend_id: None,
            context_size: None,
            max_output_tokens: None,
            draft_model: None,
            sampling: SamplingConfig::default(),
            repeat_penalty: 1.0,
            repeat_last_n: 64,
            frequency_penalty: 0.0,
            presence_penalty: 0.0,
            n_batch: None,
            n_gpu_layers: None,
            use_mlock: false,
            flash_attention: None,
            n_threads: None,
            tool_calling: ToolCallingMode::Auto,
            chat_template: ChatTemplate::Embedded,
            enable_thinking: true,
            vision_capable: false,
            image_token_estimate: default_image_token_estimate(),
            mmproj_size_bytes: 0,
        }
    }
}
