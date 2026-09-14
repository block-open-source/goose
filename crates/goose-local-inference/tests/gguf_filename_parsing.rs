//! Filename shapes captured from real HuggingFace repos, to keep GGUF discovery
//! working across publishers that disagree about where the quantization tag goes.

#![cfg(feature = "hf-hub")]

use goose_local_inference::hf_models::{is_auxiliary_gguf_file, parse_quantization_from_filename};

fn quant(filename: &str) -> String {
    parse_quantization_from_filename(filename)
}

#[test]
fn parses_conventional_trailing_quant_tags() {
    assert_eq!(quant("Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf"), "Q4_K_M");
    assert_eq!(quant("Qwen3.6-27B-IQ4_NL.gguf"), "IQ4_NL");
    assert_eq!(quant("Qwen3.6-35B-A3B-MXFP4_MOE.gguf"), "MXFP4_MOE");
    assert_eq!(quant("Model-UD-IQ1_M.gguf"), "IQ1_M");
    assert_eq!(quant("Model-UD-Q4_K_XL.gguf"), "Q4_K_XL");
}

#[test]
fn parses_quant_tags_buried_mid_filename() {
    // google/gemma-4-*-it-qat-q4_0-gguf publish the tag before the "-it" suffix.
    assert_eq!(quant("gemma-4-26B_q4_0-it.gguf"), "Q4_0");
    assert_eq!(quant("gemma-4-31B_q4_0-it.gguf"), "Q4_0");
    assert_eq!(quant("gemma-4-E4B_q4_0-it.gguf"), "Q4_0");
    // google/gemma-3-*-it-qat-q4_0-gguf drop the "qat" segment the repo name carries.
    assert_eq!(quant("gemma-3-27b-it-q4_0.gguf"), "Q4_0");
    // Qwen/Qwen3-VL-30B-A3B-Instruct-GGUF interleaves a "split" marker.
    assert_eq!(
        quant("Qwen3VL-30B-A3B-Instruct-F16-split-00001-of-00002.gguf"),
        "F16"
    );
}

#[test]
fn canonicalizes_quant_case_so_lookups_match_the_quant_table() {
    assert_eq!(quant("Model-q4_k_m.gguf"), "Q4_K_M");
    assert_eq!(quant("Gemma-3-1B-Heretic_Q3_k_m.gguf"), "Q3_K_M");
}

#[test]
fn strips_shard_and_directory_prefixes() {
    assert_eq!(quant("Q5_K_M/Model-Q5_K_M-00001-of-00002.gguf"), "Q5_K_M");
    assert_eq!(quant("BF16/Qwen3.6-27B-BF16-00001-of-00002.gguf"), "BF16");
}

#[test]
fn does_not_match_a_quant_tag_inside_a_longer_token() {
    // "F16" sits inside "BF16"; "Q2_K" sits inside "Q2_K_L".
    assert_eq!(quant("mmproj-BF16.gguf"), "BF16");
    assert_eq!(quant("Model-Q2_K_L.gguf"), "Q2_K_L");
    assert_eq!(quant("Model-Q3_K_XL.gguf"), "Q3_K_XL");
}

#[test]
fn preserves_quant_tags_that_are_absent_from_the_quant_table() {
    // Matching must not degrade these to the shorter table entry inside them.
    assert_eq!(quant("Model-Q6_K_L.gguf"), "Q6_K_L");
    assert_eq!(quant("Model-Q5_K_L.gguf"), "Q5_K_L");
    assert_eq!(quant("Model-Q4_0_4_4.gguf"), "Q4_0_4_4");
    assert_eq!(quant("Model-Q2_K_P.gguf"), "Q2_K_P");
    assert_eq!(quant("ggml-model-Q3_K.gguf"), "Q3_K");
}

#[test]
fn reports_unknown_when_no_quant_tag_is_present() {
    assert_eq!(quant("random-name.gguf"), "unknown");
    assert_eq!(quant("imatrix.gguf"), "unknown");
    assert_eq!(quant("surya-2.gguf"), "unknown");
    // A quant tag's family prefix must be followed by a digit, so a model name
    // is never mistaken for a quantization by the component scan.
    assert_eq!(quant("Qwen3.6-27B-Instruct.gguf"), "unknown");
}

#[test]
fn keeps_named_presets_in_the_tag_position_as_variant_labels() {
    // "APEX"/"OPAL" builds ship a preset name where the quant tag normally sits.
    // These are the only variant these repos expose, so they stay listed.
    assert_eq!(quant("Qwen3.6-35B-A3B-APEX-I-Quality.gguf"), "Quality");
    assert_eq!(quant("SIQ-1-35B-OPAL-quality.gguf"), "quality");
    assert_eq!(quant("Model.Quality.gguf"), "Quality");
    // A real quant tag elsewhere in the name still wins over the preset word.
    assert_eq!(quant("Model-Q4_0-quality.gguf"), "Q4_0");
}

#[test]
fn parses_dot_separated_quant_tags() {
    assert_eq!(
        quant("DeepSeek-V3-0324.IQ1_M.gguf-00001-of-00009.gguf"),
        "IQ1_M"
    );
    assert_eq!(quant("model.Q4_K_M.gguf"), "Q4_K_M");
}

#[test]
fn treats_projectors_and_encoders_as_auxiliary() {
    assert!(is_auxiliary_gguf_file("mmproj-BF16.gguf"));
    assert!(is_auxiliary_gguf_file("gemma-4-26B-it-mmproj.gguf"));
    assert!(is_auxiliary_gguf_file("mmproj-model-f16-27B.gguf"));
    assert!(is_auxiliary_gguf_file("vision-encoder-Q4_K_M.gguf"));
    assert!(is_auxiliary_gguf_file("Q4_K_M/mmproj-F32.gguf"));
    assert!(is_auxiliary_gguf_file("draft/Model-Q4_K_M.gguf"));
    assert!(is_auxiliary_gguf_file("adapter/Model-Q4_K_M.gguf"));
    assert!(is_auxiliary_gguf_file("lora/Model-Q4_K_M.gguf"));
}

#[test]
fn treats_mtp_drafters_as_auxiliary_but_keeps_mtp_named_models() {
    // Drafters: leading "mtp-" basename, or an "MTP/" directory.
    assert!(is_auxiliary_gguf_file(
        "MTP/mtp-gemma-4-26B-A4B-it-BF16.gguf"
    ));
    assert!(is_auxiliary_gguf_file("mtp-Qwen3.6-35B-A3B-BF16.gguf"));
    assert!(is_auxiliary_gguf_file("mtp-gemma-4-26B-A4B-it.gguf"));

    // Real weights whose model name happens to contain "MTP".
    assert!(!is_auxiliary_gguf_file(
        "Qwopus3.6-27B-Coder-MTP-Q3_K_M.gguf"
    ));
    assert!(!is_auxiliary_gguf_file("Ornith-1.0-9B-MTP-BF16.gguf"));
    assert!(!is_auxiliary_gguf_file(
        "Qwen3.6-27B-Fable-Fus-711-NEO-MAX-MTP-IQ4_XS.gguf"
    ));
}

#[test]
fn keeps_ordinary_model_files() {
    assert!(!is_auxiliary_gguf_file("gemma-4-26B_q4_0-it.gguf"));
    assert!(!is_auxiliary_gguf_file("gemma-3-27b-it-Q4_K_M.gguf"));
    assert!(!is_auxiliary_gguf_file(
        "BF16/gemma-3-27b-it-BF16-00001-of-00002.gguf"
    ));
}
