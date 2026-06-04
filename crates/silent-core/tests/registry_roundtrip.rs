//! Round-trip the registry types against a representative TOML entry modelled on
//! the PRD Appendix B sketch. This is the contract Task D1 (registry population)
//! writes data against, so it must parse every R4 field and survive a
//! TOML → struct → TOML → struct round-trip with no loss.

// Tests use `expect()` as the assertion mechanism: a parse/serialize failure
// SHOULD panic the test with a message. The PRD lint config explicitly allows
// this in tests ("production paths; tests may allow").
#![allow(clippy::expect_used, clippy::unwrap_used)]

use silent_core::ids::ModelId;
use silent_core::registry::{CacheStore, ExecutionProvider, Host, Provider, Registry, Task};

/// The Appendix B Nemotron + Voxtral sketch, adapted so `cache` is the
/// table form (`[model.cache] store = "..."`) the typed `Cache` parses. The
/// `device_tiers` use the exact tier names from Appendix B.
const SAMPLE_TOML: &str = r#"
[[model]]
id = "asr.nemotron.streaming_0_6b"
task = "asr"
provider = "huggingface"
repo = "FluffyBunnies/nemotron-streaming-0_6b-onnx"
revision = "0123456789abcdef0123456789abcdef01234567"
host = "rust-ort-web"
execution_provider = "cpu"
precision = ["int8"]
memory_budget_mb = 1400
license = "nvidia-open-model-license"
license_verified = true
network_origins = ["https://huggingface.co", "https://cdn-lfs.huggingface.co"]

  [model.cache]
  store = "cache-api"
  verify_once_per_revision = true

  [[model.files]]
  path = "encoder.onnx"
  size = 924000000
  sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  purpose = "encoder"

  [[model.files]]
  path = "decoder_joint_fp32.onnx"
  size = 37700000
  sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  purpose = "decoder"

  [[model.files]]
  path = "tokenizer.model"
  size = 251000
  sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
  purpose = "tokenizer"

  [model.device_tiers.wasm_only]
  default_for_tier = true

  [model.device_tiers.webgpu_low]
  default_for_tier = true

  [model.validation]
  golden_fixtures = ["nemotron/test_16k"]
  expected = "100% word accuracy on the golden clip"

  [model.perf_budget]
  ttft_ms_max = 1000
  rtf_browser_max = 0.5
  rtf_native_max = 0.2

[[model]]
id = "asr.voxtral.realtime_4b"
task = "asr"
provider = "huggingface"
repo = "onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX"
revision = "fedcba9876543210fedcba9876543210fedcba98"
host = "js-transformers"
execution_provider = "webgpu"
precision = ["q4f16"]
memory_budget_mb = 5500
license = "apache-2.0"
license_verified = true
network_origins = ["https://huggingface.co", "https://cdn-lfs.huggingface.co"]

  [model.cache]
  store = "transformers-idb"

  [model.device_tiers.webgpu_high]
  default_for_tier = true
  min_memory_gb = 16
  requires_webgpu = true

  [model.perf_budget]
  flat_memory_10min = true
"#;

#[test]
fn parses_every_r4_field() {
    let reg: Registry = toml::from_str(SAMPLE_TOML).expect("sample registry parses");
    assert_eq!(reg.models.len(), 2, "two models in the sample");

    let nemo = reg
        .get(&ModelId::new("asr.nemotron.streaming_0_6b"))
        .expect("nemotron entry present");

    // Scalars / enums.
    assert_eq!(nemo.task, Task::Asr);
    assert_eq!(nemo.provider, Provider::Huggingface);
    assert_eq!(nemo.host, Host::RustOrtWeb);
    assert_eq!(nemo.execution_provider, ExecutionProvider::Cpu);
    assert_eq!(nemo.precision, vec!["int8".to_owned()]);
    assert_eq!(nemo.memory_budget_mb, 1400);
    assert_eq!(nemo.license, "nvidia-open-model-license");
    assert!(nemo.license_verified);
    assert!(
        nemo.revision.len() == 40,
        "revision is a 40-char commit sha"
    );

    // network_origins.
    assert_eq!(nemo.network_origins.len(), 2);

    // files: path / size / sha256 / purpose.
    assert_eq!(nemo.files.len(), 3);
    let enc = &nemo.files[0];
    assert_eq!(enc.path, "encoder.onnx");
    assert_eq!(enc.size, Some(924_000_000));
    assert_eq!(enc.sha256.as_deref().map(str::len), Some(64));
    assert_eq!(enc.purpose.as_deref(), Some("encoder"));

    // cache.
    assert_eq!(nemo.cache.store, CacheStore::CacheApi);
    assert!(nemo.cache.verify_once_per_revision);

    // device_tiers.
    assert_eq!(nemo.device_tiers.len(), 2);
    assert!(nemo.device_tiers["wasm_only"].default_for_tier);

    // validation.
    let v = nemo.validation.as_ref().expect("validation present");
    assert_eq!(v.golden_fixtures, vec!["nemotron/test_16k".to_owned()]);

    // perf budget.
    let p = nemo.perf_budget.as_ref().expect("perf budget present");
    assert_eq!(p.ttft_ms_max, Some(1000));
    assert_eq!(p.rtf_browser_max, Some(0.5));
    assert_eq!(p.rtf_native_max, Some(0.2));

    // Voxtral: webgpu tier with memory + webgpu requirement, idb cache, flat mem.
    let vox = reg
        .get(&ModelId::new("asr.voxtral.realtime_4b"))
        .expect("voxtral entry present");
    assert_eq!(vox.host, Host::JsTransformers);
    assert_eq!(vox.execution_provider, ExecutionProvider::Webgpu);
    assert_eq!(vox.cache.store, CacheStore::TransformersIdb);
    assert!(
        vox.cache.verify_once_per_revision,
        "verify_once_per_revision defaults to true when omitted"
    );
    let tier = &vox.device_tiers["webgpu_high"];
    assert!(tier.default_for_tier);
    assert_eq!(tier.min_memory_gb, Some(16));
    assert_eq!(tier.requires_webgpu, Some(true));
    assert_eq!(
        vox.perf_budget.as_ref().and_then(|p| p.flat_memory_10min),
        Some(true)
    );
}

#[test]
fn round_trips_through_toml_without_loss() {
    let parsed: Registry = toml::from_str(SAMPLE_TOML).expect("first parse");
    let serialized = toml::to_string(&parsed).expect("serialize back to toml");
    let reparsed: Registry = toml::from_str(&serialized).expect("re-parse serialized toml");
    assert_eq!(
        parsed, reparsed,
        "registry survives a toml -> struct -> toml -> struct round-trip"
    );
}

#[test]
fn round_trips_through_json_without_loss() {
    // JSON is the wire format for the egress manifest / in-app license display.
    let parsed: Registry = toml::from_str(SAMPLE_TOML).expect("parse toml");
    let json = serde_json::to_string(&parsed).expect("to json");
    let from_json: Registry = serde_json::from_str(&json).expect("from json");
    assert_eq!(parsed, from_json, "registry survives a json round-trip");
}
