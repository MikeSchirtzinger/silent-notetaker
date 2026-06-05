//! The Hugging Face model registry types (PRD R4).
//!
//! The repo stores typed model metadata — never weights. The registry is the
//! single source of truth: engine selection, CSP generation, the egress
//! manifest, license display, and cache verification all derive from it. These
//! types parse the TOML registry (Appendix B sketch) and are the contract
//! `xtask model-audit` / `gen-headers` / `deploy-gate` (Task D2) read.
//!
//! Every R4 field is represented: `id`, `task`, `provider`, `repo`, `revision`,
//! `files` (path/size/sha256/purpose), `host`, `execution_provider`,
//! `precision`, `device_tiers`, `memory_budget_mb`, `cache` (+ hash-verification
//! policy), `license`, `license_verified`, `network_origins`, `validation`, and
//! the R9 perf budgets.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::ModelId;

/// The top-level registry document: `[[model]]` array of entries.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Registry {
    /// Every registered model. Parsed from `[[model]]` TOML tables.
    #[serde(default, rename = "model")]
    pub models: Vec<Model>,
}

impl Registry {
    /// Find a model entry by id.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&Model> {
        self.models.iter().find(|m| &m.id == id)
    }
}

/// One model in the registry (PRD R4 "Registry fields").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Model {
    /// Stable local id, for example `asr.nemotron.streaming_0_6b`.
    pub id: ModelId,

    /// What the model does.
    pub task: Task,

    /// Artifact provider (currently always Hugging Face).
    pub provider: Provider,

    /// Hugging Face repo, for example `onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX`.
    pub repo: String,

    /// Exact commit SHA or immutable revision. `main` is **not acceptable** for
    /// production defaults (enforced by `xtask model-audit`, not by this type —
    /// the registry must still parse a bad value to report it).
    pub revision: String,

    /// Which runtime executes the model.
    pub host: Host,

    /// CPU or WebGPU (per host availability).
    pub execution_provider: ExecutionProvider,

    /// Available dtype variants (`fp32`, `fp16`, `q8`, `q4f16`, `int8`, …).
    pub precision: Vec<String>,

    /// Expected peak memory budget, megabytes.
    pub memory_budget_mb: u32,

    /// Cache + hash-verification policy.
    pub cache: Cache,

    /// Upstream license identifier (SPDX where one exists, else a descriptive
    /// tag like `nvidia-open-model-license`).
    pub license: String,

    /// `true` only after a human has read the license (PRD R4; A2 report). A
    /// shipped default must have this `true`.
    pub license_verified: bool,

    /// Derived allowlist entries needed to fetch this model's artifacts. Feeds
    /// CSP `connect-src` generation (Task D2).
    #[serde(default)]
    pub network_origins: Vec<String>,

    /// Required artifacts, each with path/size/sha256/purpose. Multi-file
    /// artifact sets are first-class (Voxtral ~2.7 GB; Nemotron three files).
    #[serde(default, rename = "files")]
    pub files: Vec<ModelFile>,

    /// Per-tier defaults and requirements (the Qwen tier-default mechanism as
    /// data). Keyed by tier name (`wasm_only`, `webgpu_high`, …).
    #[serde(default)]
    pub device_tiers: BTreeMap<String, DeviceTier>,

    /// Golden fixture ids and expected outputs for this model.
    #[serde(default)]
    pub validation: Option<Validation>,

    /// R9 performance budgets enforced as regression gates.
    #[serde(default)]
    pub perf_budget: Option<PerfBudget>,

    /// Settings-picker presentation, when this model is a user-selectable ASR
    /// engine (PRD Phase 5 / R3; Appendix A row 7). `None` for entries that are
    /// not directly user-picked in the ASR slot (TitaNet embedder, the Qwen
    /// notes models — those have their own surfaces). The picker (Task I3 /
    /// `silent_inference::selection`) renders one option per `Some(ui)`, ordered
    /// by [`ModelUi::order`]; the label and the persisted `value` come from
    /// here, so adding a Whisper size is a registry entry — zero code (R3
    /// acceptance).
    #[serde(default)]
    pub ui: Option<ModelUi>,

    /// For a **composite** engine (Dual = Moonshine drafts + SenseVoice refiner),
    /// the ids of the underlying model entries it reuses, in policy order
    /// (`[draft, refine]`). Empty for ordinary single-model entries. A composite
    /// entry carries no own `files`; the selection module resolves artifacts and
    /// availability by following these ids (Appendix A row 11).
    #[serde(default)]
    pub composite_of: Vec<ModelId>,
}

/// Settings-picker presentation for a user-selectable ASR engine (the `value`
/// persisted in `localStorage` + the option label shown), kept as registry data
/// so the picker stays "registry entry = zero code" (PRD R3).
///
/// `value` is the stable selection key the UI persists (e.g. `voxtral`,
/// `nemotron`, an HF repo id for the Whisper/Moonshine families) — it is NOT the
/// registry [`Model::id`], because the shipping UI and `localStorage` already use
/// these short keys and that contract must not break (pixel-identical UX).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ModelUi {
    /// The picker option `value` the UI persists in `localStorage` under
    /// `settings.model` (e.g. `voxtral`, `nemotron`, `dual`, `sensevoice`, or an
    /// HF repo id like `onnx-community/whisper-small.en`). Load-bearing: the
    /// shipping `index.html` branches on these exact strings.
    pub value: String,

    /// The exact option label shown in the picker (must match the shipping
    /// option text verbatim for pixel-identical UX).
    pub label: String,

    /// Position in the picker list (ascending). The shipping order is preserved
    /// by these values; ties fall back to registry order.
    pub order: u32,
}

/// The task a model performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Task {
    /// Automatic speech recognition.
    Asr,
    /// Speaker embedding (diarization).
    SpeakerEmbedding,
    /// Notes / smart-question generation.
    Notes,
    /// Voice-activity detection.
    Vad,
}

/// Artifact provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Provider {
    /// Hugging Face.
    Huggingface,
}

/// The runtime that executes a model (the hybrid is honest because the registry
/// records it — PRD R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Host {
    /// Rust core driving `ort-web` (Nemotron, TitaNet).
    RustOrtWeb,
    /// Rust policy driving a transformers.js worker (Voxtral, Whisper,
    /// Moonshine, Qwen).
    JsTransformers,
    /// The sherpa-onnx Emscripten runtime (SenseVoice).
    JsSherpa,
}

/// Execution provider (per host availability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExecutionProvider {
    /// CPU / WASM.
    Cpu,
    /// WebGPU execution provider.
    Webgpu,
}

/// One required artifact file (PRD R4 `files`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct ModelFile {
    /// Path within the repo, for example `encoder.onnx`.
    pub path: String,

    /// File size in bytes (from the HF API LFS oid — never download multi-GB
    /// files to hash). Optional only because some entries are pinned before
    /// sizes are recorded; `xtask model-audit` requires it on shipped defaults.
    #[serde(default)]
    pub size: Option<u64>,

    /// sha256 of the file contents. Optional for the same reason as `size`;
    /// required on shipped defaults.
    #[serde(default)]
    pub sha256: Option<String>,

    /// What the artifact is for (free-form: `encoder`, `decoder`, `tokenizer`,
    /// `wasm-runtime`, …). Optional.
    #[serde(default)]
    pub purpose: Option<String>,
}

/// Cache + hash-verification policy (PRD R4 `cache`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Cache {
    /// Where artifacts are cached.
    pub store: CacheStore,

    /// Hash-verification policy: verify once per revision, record it, and do
    /// **not** re-hash multi-GB files on every load. `true` is the default;
    /// set `false` only for tiny fixtures where per-load verification is cheap.
    #[serde(default = "default_verify_once_per_revision")]
    pub verify_once_per_revision: bool,
}

fn default_verify_once_per_revision() -> bool {
    true
}

/// Cache backend (PRD R4: `cache-api` or `transformers-idb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CacheStore {
    /// The browser Cache API (ort-web / sherpa hosts).
    CacheApi,
    /// transformers.js's own IndexedDB cache.
    TransformersIdb,
}

/// A `Cache` may be given in TOML as a bare string (`cache = "cache-api"`) or as
/// a table (`[model.cache] store = "cache-api"`). We accept the bare-string form
/// for ergonomics by defaulting the policy; the Appendix B sketch uses the bare
/// string, so this keeps it round-trippable. Implemented via untagged enum on
/// the wire is avoided to keep ts-rs output clean — instead `Cache` deserializes
/// from a table and `CacheStore` from the string; the registry data files use
/// the table form, and the round-trip test exercises it.
impl Cache {
    /// Construct a cache policy with the default verify-once-per-revision.
    #[must_use]
    pub fn new(store: CacheStore) -> Self {
        Self {
            store,
            verify_once_per_revision: true,
        }
    }
}

/// Per-tier defaults and requirements (PRD R4 `device_tiers`; encodes the Qwen
/// tier-default mechanism as data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct DeviceTier {
    /// Whether this model is the default choice for this device tier.
    #[serde(default)]
    pub default_for_tier: bool,

    /// Minimum system memory required, gigabytes (Voxtral needs 16).
    #[serde(default)]
    pub min_memory_gb: Option<u32>,

    /// Whether this tier requires WebGPU.
    #[serde(default)]
    pub requires_webgpu: Option<bool>,
}

/// Golden-fixture validation metadata (PRD R4 `validation`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Validation {
    /// Golden fixture ids exercised against this model (PRD "Golden tests":
    /// every engine ships `goldens/<engine>/`).
    #[serde(default)]
    pub golden_fixtures: Vec<String>,

    /// Expected output for the golden fixture, where deterministic (a golden
    /// transcript, or a cosine threshold expressed as text). Free-form per the
    /// fixture; brittle exact strings are avoided unless deterministic.
    #[serde(default)]
    pub expected: Option<String>,
}

/// R9 performance budgets, enforced as regression gates in the golden harness.
///
/// All fields optional: a model declares only the gates that apply to it (an
/// embedder has no TTFT). The golden harness fails the build when a measured
/// value exceeds a declared budget.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct PerfBudget {
    /// Max time-to-first-text in the browser, milliseconds (Nemotron: 1000).
    #[serde(default)]
    pub ttft_ms_max: Option<u32>,

    /// Max real-time factor in the browser WASM path (Nemotron: 0.5).
    #[serde(default)]
    pub rtf_browser_max: Option<f32>,

    /// Max real-time factor on the native path (Nemotron: 0.2).
    #[serde(default)]
    pub rtf_native_max: Option<f32>,

    /// Whether a 10-minute session must hold flat memory (Voxtral two-cap
    /// recycle; all engines: no unbounded heap growth).
    #[serde(default)]
    pub flat_memory_10min: Option<bool>,
}
