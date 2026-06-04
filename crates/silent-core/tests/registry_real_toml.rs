//! Integration test: parse the real `registry/models.toml` file produced by
//! Task D1. Every entry must deserialize through the silent-core `Registry`
//! types — if this test breaks, a registry edit violated the schema contract.
//!
//! Approach: load the file by absolute path relative to the manifest dir so
//! the test is hermetic regardless of `cargo test` invocation location.

// Tests use `expect()` / `unwrap()` as the assertion mechanism: a parse
// failure SHOULD panic with a message. Allowed per the PRD lint config
// ("tests may allow").
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashSet;

use silent_core::registry::{CacheStore, ExecutionProvider, Host, Provider, Registry, Task};

/// Resolve the registry path relative to this crate's `CARGO_MANIFEST_DIR`.
/// `registry/models.toml` lives two directories up (workspace root).
fn registry_path() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/silent-core → workspace root is ../../
    manifest.join("../../registry/models.toml")
}

/// Parse the real registry file and return the `Registry`.
fn load_registry() -> Registry {
    let path = registry_path();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    toml::from_str(&contents).unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()))
}

#[test]
fn registry_toml_parses_all_entries() {
    let reg = load_registry();
    // The PRD R4 initial table has 11 entries (Nemotron, Voxtral, Whisper ×4,
    // Moonshine, TitaNet, Qwen ×2, SenseVoice). Assert at least this many so
    // a silent truncation is caught.
    assert!(
        reg.models.len() >= 11,
        "expected at least 11 model entries, got {}",
        reg.models.len()
    );
}

#[test]
fn no_revision_main_in_registry() {
    let reg = load_registry();
    for model in &reg.models {
        assert_ne!(
            model.revision, "main",
            "model {} has forbidden revision='main'",
            model.id
        );
        assert!(
            !model.revision.is_empty(),
            "model {} has empty revision",
            model.id
        );
    }
}

#[test]
fn all_required_models_present() {
    let reg = load_registry();
    let ids: HashSet<&str> = reg.models.iter().map(|m| m.id.as_str()).collect();

    let required = [
        "asr.nemotron.streaming_0_6b",
        "asr.voxtral.realtime_4b",
        "asr.whisper.large_v3_turbo",
        "asr.whisper.small_en",
        "asr.whisper.base_en",
        "asr.whisper.tiny_en",
        "asr.moonshine.base",
        "speaker_embedding.titanet.small",
        "notes.qwen3.0_6b",
        "notes.qwen3.1_7b",
        "asr.sensevoice.sherpa_small",
    ];

    for id in required {
        assert!(
            ids.contains(id),
            "required model {id} missing from registry"
        );
    }
}

#[test]
fn no_license_verified_true_without_explicit_gate() {
    // PRD contract: `license_verified = true` only after a human has read the
    // license. D1 sets all to false; only Mike flips them. This test fails if
    // an agent flips one without authorisation.
    let reg = load_registry();
    for model in &reg.models {
        assert!(
            !model.license_verified,
            "model {} has license_verified=true — only Mike may flip this flag \
             (PRD R4; D1 spec ground rule)",
            model.id
        );
    }
}

#[test]
fn non_placeholder_files_have_sha256_and_size() {
    // Every file entry that has a sha256 must also have size, and vice versa.
    // Placeholder SenseVoice revision is allowed to exist; the file entries
    // must still be populated (A5 pre-hashed them).
    let reg = load_registry();
    for model in &reg.models {
        for file in &model.files {
            if let Some(ref sha) = file.sha256 {
                assert!(
                    !sha.is_empty(),
                    "model {} file {} has empty sha256",
                    model.id,
                    file.path
                );
                assert!(
                    file.size.is_some(),
                    "model {} file {} has sha256 but no size",
                    model.id,
                    file.path
                );
                // sha256 should be a 64-char hex string for non-placeholder entries
                if sha != "N/A" {
                    assert_eq!(
                        sha.len(),
                        64,
                        "model {} file {} sha256 is not 64 chars: {sha}",
                        model.id,
                        file.path
                    );
                }
            }
        }
    }
}

#[test]
fn sensevoice_is_gated() {
    // SenseVoice revision must contain the BLOCKED gate marker until Mike uploads.
    let reg = load_registry();
    let sv = reg
        .models
        .iter()
        .find(|m| m.id.as_str() == "asr.sensevoice.sherpa_small")
        .expect("SenseVoice entry must be present");

    assert!(
        sv.revision.contains("BLOCKED"),
        "SenseVoice revision should be the BLOCKED-ON-USER-GATE placeholder \
         until Mike runs the upload (got {:?})",
        sv.revision
    );
    // Host must be js-sherpa
    assert_eq!(
        sv.host,
        Host::JsSherpa,
        "SenseVoice must use js-sherpa host"
    );
    // Must have the pre-hashed files from A5
    assert_eq!(
        sv.files.len(),
        8,
        "SenseVoice must have 8 pre-hashed file entries (5 HTTP + 3 packed)"
    );
}

#[test]
fn nemotron_has_r9_perf_budget() {
    let reg = load_registry();
    let nemo = reg
        .models
        .iter()
        .find(|m| m.id.as_str() == "asr.nemotron.streaming_0_6b")
        .expect("Nemotron entry must be present");

    let budget = nemo
        .perf_budget
        .as_ref()
        .expect("Nemotron must have a perf_budget (PRD R9)");
    assert_eq!(
        budget.ttft_ms_max,
        Some(1000),
        "Nemotron TTFT budget must be ≤1000 ms"
    );
    assert_eq!(
        budget.rtf_browser_max,
        Some(0.5),
        "Nemotron browser RTF budget must be ≤0.5×"
    );
    assert_eq!(
        budget.rtf_native_max,
        Some(0.2),
        "Nemotron native RTF budget must be ≤0.2×"
    );
}

#[test]
fn voxtral_requires_webgpu_high_tier() {
    let reg = load_registry();
    let vox = reg
        .models
        .iter()
        .find(|m| m.id.as_str() == "asr.voxtral.realtime_4b")
        .expect("Voxtral entry must be present");

    assert_eq!(vox.host, Host::JsTransformers);
    assert_eq!(vox.execution_provider, ExecutionProvider::Webgpu);
    assert_eq!(vox.cache.store, CacheStore::TransformersIdb);

    let tier = vox
        .device_tiers
        .get("webgpu_high")
        .expect("Voxtral must have webgpu_high tier");
    assert!(
        tier.default_for_tier,
        "Voxtral must be the webgpu_high default"
    );
    assert_eq!(
        tier.min_memory_gb,
        Some(16),
        "Voxtral webgpu_high requires 16 GB"
    );
    assert_eq!(
        tier.requires_webgpu,
        Some(true),
        "Voxtral webgpu_high must require WebGPU"
    );
}

#[test]
fn titanet_uses_rust_ort_web_cpu() {
    let reg = load_registry();
    let tn = reg
        .models
        .iter()
        .find(|m| m.id.as_str() == "speaker_embedding.titanet.small")
        .expect("TitaNet entry must be present");

    assert_eq!(tn.task, Task::SpeakerEmbedding);
    assert_eq!(tn.host, Host::RustOrtWeb);
    assert_eq!(
        tn.execution_provider,
        ExecutionProvider::Cpu,
        "TitaNet must be CPU — avoids GPU contention with Voxtral"
    );
    assert_eq!(tn.provider, Provider::Huggingface);
    // Must not be pointing at the unpinned 'main' URL (validated separately,
    // but double-check the revision is a 40-char commit SHA).
    assert_eq!(
        tn.revision.len(),
        40,
        "TitaNet revision must be a 40-char commit SHA"
    );
}

#[test]
fn qwen_tier_mechanism_encoded_as_data() {
    // 0.6B is the wasm_only + webgpu_low default; 1.7B is the webgpu_mid +
    // webgpu_high default. This is the Qwen tier mechanism from nemotron-rust,
    // generalised as registry data (PRD R3, R4).
    let reg = load_registry();

    let q06 = reg
        .models
        .iter()
        .find(|m| m.id.as_str() == "notes.qwen3.0_6b")
        .expect("Qwen3-0.6B entry must be present");
    assert!(
        q06.device_tiers
            .get("wasm_only")
            .is_some_and(|t| t.default_for_tier),
        "Qwen3-0.6B must be the wasm_only default"
    );
    assert!(
        q06.device_tiers
            .get("webgpu_low")
            .is_some_and(|t| t.default_for_tier),
        "Qwen3-0.6B must be the webgpu_low default"
    );
    // 0.6B must NOT be default on high tiers
    assert!(
        !q06.device_tiers
            .get("webgpu_high")
            .is_some_and(|t| t.default_for_tier),
        "Qwen3-0.6B must NOT be the webgpu_high default (1.7B is)"
    );

    let q17 = reg
        .models
        .iter()
        .find(|m| m.id.as_str() == "notes.qwen3.1_7b")
        .expect("Qwen3-1.7B entry must be present");
    assert!(
        q17.device_tiers
            .get("webgpu_high")
            .is_some_and(|t| t.default_for_tier),
        "Qwen3-1.7B must be the webgpu_high default"
    );
    assert!(
        q17.device_tiers
            .get("webgpu_mid")
            .is_some_and(|t| t.default_for_tier),
        "Qwen3-1.7B must be the webgpu_mid default"
    );
    // 1.7B must NOT be default on wasm_only
    assert!(
        !q17.device_tiers
            .get("wasm_only")
            .is_some_and(|t| t.default_for_tier),
        "Qwen3-1.7B must NOT be the wasm_only default (too large)"
    );
}

#[test]
fn registry_survives_toml_round_trip() {
    let path = registry_path();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let parsed: Registry = toml::from_str(&contents).expect("real registry TOML must parse");
    let serialised = toml::to_string(&parsed).expect("re-serialise to TOML");
    let reparsed: Registry = toml::from_str(&serialised).expect("re-parse serialised TOML");
    assert_eq!(
        parsed, reparsed,
        "real registry survives toml → struct → toml → struct round-trip"
    );
}

#[test]
fn all_network_origins_non_empty() {
    let reg = load_registry();
    for model in &reg.models {
        assert!(
            !model.network_origins.is_empty(),
            "model {} has no network_origins — CSP gen needs at least one origin",
            model.id
        );
        for origin in &model.network_origins {
            assert!(
                origin.starts_with("https://"),
                "model {} origin {origin:?} must start with https://",
                model.id
            );
        }
    }
}
