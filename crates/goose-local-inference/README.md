# goose-local-inference

On-device model inference for goose. Runs GGUF models through `llama.cpp` (via
`llama-cpp-2`), with an optional MLX backend on Apple silicon.

Reach it through [`goose-providers`](../goose-providers) with the
`local-inference` feature, which exposes `LocalInferenceProvider` as an ordinary
`Provider`.

## Features

Default is `[]` — CPU inference.

- `hf-hub` — Hugging Face model discovery, downloads, cache inventory, and
  management APIs. Without it, models can still be loaded directly from paths.
- `cuda`, `vulkan` — GPU acceleration via the corresponding `llama-cpp-2` backend.
- `mlx` — the MLX backend for Apple silicon.

## What it handles

- **Runtime and placement** — `InferenceRuntime` describes the machine and
  `available_inference_memory_bytes` helps choose a cached model that will fit.
- **Model lifecycle** — `is_model_loaded`, `loaded_model_ids`, and `evict_model`
  manage what's resident. With `hf-hub` enabled, `hf_models` uses the Hugging
  Face cache as the model inventory, `management` exposes it to clients, and
  `huggingface_auth` handles gated repos.
- **Prompt formatting** — `prompt_template` applies the model's chat template;
  `builtin_chat_template_names()` lists the bundled ones.
- **Tool calling** — `native_tool_parsing` and `tool_parsing` extract tool calls
  from model output, and `tool_emulation` (toolshim) fills in for models with no
  native tool support.
- **Richer outputs** — `thinking_output` separates reasoning blocks from the
  answer; `multimodal` handles image input.
- **Config** — `config_resolver` and `provider_utils` resolve settings such as
  `LOCAL_LLM_MODEL`.

## Loading models from a path

Set the model name to a local path to bypass the Hugging Face cache. A `.gguf`
file uses the llama.cpp backend. An MLX model directory containing
`config.json`, `tokenizer.json`, and SafeTensors weights uses the MLX backend;
a path to one of its `.safetensors` files is accepted as well. Relative paths
are resolved from the process working directory.

Models loaded this way remain user-owned: Goose can load and evict them from
memory, but does not include them in the cached-model inventory or delete their
files.

## Building

The `llama.cpp` backends compile native code, so a C/C++ toolchain is required,
plus the CUDA or Vulkan SDK when selecting those features.

```bash
cargo build -p goose-local-inference
cargo build -p goose-local-inference --features hf-hub
cargo build -p goose-local-inference --features mlx
```
