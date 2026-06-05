//! Registry-driven engine selection — the model picker's policy, in Rust
//! (PRD Phase 5 / R3; Appendix A rows 7, 8).
//!
//! This is the home of everything the settings model picker needs to be
//! **registry-driven** instead of a hand-maintained JS table:
//!
//! - the embedded registry (the single source of truth, R4);
//! - the [`DeviceProbe`] — a typed device-capability struct the JS hands in
//!   (WebGPU available, memory, `crossOriginIsolated`, thread count), and the
//!   [`DeviceTier`] it maps to (the Qwen tier mechanism, generalized — R3);
//! - the picker option list ([`asr_picker_options`]) sourced from registry
//!   `ui` entries so every shipping engine (incl. Nemotron) appears exactly as
//!   today — *and adding a model is a registry entry, zero code* (R3 acceptance);
//! - availability **verdicts with reasons** ([`availability`]): an engine that
//!   needs a capability the device lacks is shown unavailable *with why* and a
//!   recommended CPU-tier alternative (R1 acceptance — never a silent fallback);
//! - the **queued mid-recording switch** policy ([`apply_selection`]): a change
//!   while recording is accepted and queued with a friendly "takes effect next
//!   meeting" notice — never a silent failure, never a hard rejection (decision
//!   log; R3 acceptance).
//!
//! # Embed-vs-deploy decision: the registry is embedded at build time
//!
//! `registry/models.toml` is `include_str!`'d into the binary and parsed once.
//! Rationale, recorded here as the decision per the task brief:
//!
//! 1. **Single source of truth, compile-checked.** A malformed registry fails
//!    the build (loud failure), not a runtime fetch. The data the selection
//!    policy reads is exactly the data CI's `xtask model-audit` / `gen-headers`
//!    validate — no drift between a deployed copy and the committed file.
//! 2. **Zero new egress / CSP surface.** Embedding means no `fetch()` for the
//!    registry, so no `connect-src` entry and no new deploy-copy step for the
//!    data. The wasm binary already ships in the deploy bundle.
//! 3. **"Registry entry = zero code" stays honest.** Adding a Whisper size is a
//!    TOML edit plus a rebuild — the picker, the tiers, and availability all
//!    follow automatically; there is no parallel JS data table to keep in sync.
//!
//! The cost is that a registry edit needs a `wasm-pack` rebuild to take effect —
//! acceptable, because every registry edit already needs the CI gate to re-run.
//!
//! # No browser dependencies
//!
//! Pure Rust law: the probe is a plain struct the JS fills in; nothing here
//! touches `navigator`/`web-sys`. It is unit-tested natively. The thin
//! wasm-bindgen surface that takes the JS probe and returns the picker JSON lives
//! in `silent-web` (Task I3 wiring).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use silent_core::ids::ModelId;
use silent_core::registry::{Host, Model, Registry, Task};

use crate::engine::{AnyAsrEngine, JsHostEngine};
use crate::voxtral_recycle::RecycleConfig;
use crate::whisper_stream::WhisperStreamConfig;

/// The registry TOML, embedded at build time (see the module docs for the
/// embed-vs-deploy decision). The path is relative to this source file:
/// `crates/silent-inference/src/selection.rs` → workspace root is `../../../`.
const EMBEDDED_REGISTRY_TOML: &str = include_str!("../../../registry/models.toml");

/// The parsed embedded registry, parsed once on first access.
///
/// Parsing is fallible (a malformed registry), but the *embedded* registry is
/// validated by CI and by silent-core's `registry_real_toml` test, so a parse
/// failure here is a build-integrity bug, not a user-facing condition. We surface
/// it as an `Err` rather than panic so callers (and the wasm surface) stay
/// panic-free on the hot path (PRD "Rust engineering bar").
fn embedded_registry() -> Result<&'static Registry, SelectionError> {
    static REGISTRY: OnceLock<Result<Registry, String>> = OnceLock::new();
    match REGISTRY.get_or_init(|| toml::from_str(EMBEDDED_REGISTRY_TOML).map_err(|e| e.to_string()))
    {
        Ok(reg) => Ok(reg),
        Err(msg) => Err(SelectionError::RegistryParse(msg.clone())),
    }
}

/// Borrow the embedded registry (the single source of truth for selection).
///
/// # Errors
///
/// Returns [`SelectionError::RegistryParse`] if the embedded TOML does not parse
/// — a build-integrity failure, caught by CI before ship.
pub fn registry() -> Result<&'static Registry, SelectionError> {
    embedded_registry()
}

/// An error from the selection policy. Loud, never silent.
///
/// A plain `Display`/`Error` impl (below) rather than a `thiserror` derive, so
/// silent-inference does not take a `thiserror` dependency just for this crate's
/// two error variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SelectionError {
    /// The embedded registry TOML failed to parse (a build-integrity bug).
    RegistryParse(String),

    /// The persisted user selection does not match any registry `ui.value` — a
    /// stale or unknown picker key. Surfaced loudly so the UI can fall back to a
    /// known default rather than silently mis-select.
    UnknownSelection(String),
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionError::RegistryParse(msg) => {
                write!(f, "embedded registry failed to parse: {msg}")
            }
            SelectionError::UnknownSelection(value) => {
                write!(f, "no registry engine for picker value `{value}`")
            }
        }
    }
}

impl std::error::Error for SelectionError {}

/// Typed device-capability probe the JS hands in (PRD R3: "device-tier detection
/// — WebGPU availability, memory, `crossOriginIsolated`, thread count").
///
/// This is the *input* to tier resolution. The JS fills it from `navigator.gpu`
/// / `navigator.deviceMemory` / `navigator.hardwareConcurrency` /
/// `crossOriginIsolated` (the shipping `GpuCaps.probe()` reads exactly these);
/// the policy that turns it into a [`DeviceTier`] is here in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeviceProbe {
    /// A real, non-fallback WebGPU adapter is available (`navigator.gpu` returned
    /// an adapter and it is not `isFallbackAdapter`). A software/fallback adapter
    /// is reported as `false` here (the JS treats it as CPU — `GpuCaps`).
    pub webgpu_available: bool,

    /// `navigator.deviceMemory` in GB, when the browser reports it (Chrome caps
    /// the report at 8). `None` when unavailable (Firefox/Safari do not expose
    /// it); tier resolution treats `None` as "unknown, assume sufficient" exactly
    /// as the shipping `GpuCaps` does (`memGB === null || memGB >= 8`).
    pub memory_gb: Option<u32>,

    /// `crossOriginIsolated` — required for the SharedArrayBuffer-backed
    /// multi-thread WASM path. When `false`, threaded WASM is unavailable and the
    /// engines fall to the single-thread path (still functional; the tier does
    /// not upgrade on thread count).
    pub cross_origin_isolated: bool,

    /// `navigator.hardwareConcurrency` (logical cores), defaulted to 4 by the JS
    /// when absent — mirrors `GpuCaps`'s `navigator.hardwareConcurrency || 4`.
    pub thread_count: u32,

    /// The adapter's `limits.maxBufferSize` in GB, when a real adapter is present
    /// (`0.0` otherwise). The shipping `GpuCaps` uses `>= 2 GB` as one of the
    /// high-tier gates ("can it even hold large weight buffers").
    pub max_gpu_buffer_gb: f32,
}

impl DeviceProbe {
    /// A conservative CPU-only probe (no WebGPU, unknown memory, 4 cores). Used
    /// as a safe default when the JS probe has not resolved yet.
    #[must_use]
    pub fn cpu_only() -> Self {
        Self {
            webgpu_available: false,
            memory_gb: None,
            cross_origin_isolated: false,
            thread_count: 4,
            max_gpu_buffer_gb: 0.0,
        }
    }
}

/// The device tier a [`DeviceProbe`] resolves to — the registry `device_tiers`
/// keys (`wasm_only`, `webgpu_low`, `webgpu_mid`, `webgpu_high`).
///
/// This is the generalization of the Qwen tier mechanism (PRD R3): a model's
/// per-tier `default_for_tier` data plus this resolution decides the recommended
/// default for the device. User choice always wins over the recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeviceTier {
    /// No usable WebGPU — CPU/WASM only.
    WasmOnly,
    /// WebGPU present but constrained (a real adapter, but not the headroom for
    /// the largest models).
    WebgpuLow,
    /// WebGPU with mid-range headroom.
    WebgpuMid,
    /// WebGPU with the headroom for the largest models (Voxtral, Qwen-1.7B).
    WebgpuHigh,
}

impl DeviceTier {
    /// The registry `device_tiers` key for this tier.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            DeviceTier::WasmOnly => "wasm_only",
            DeviceTier::WebgpuLow => "webgpu_low",
            DeviceTier::WebgpuMid => "webgpu_mid",
            DeviceTier::WebgpuHigh => "webgpu_high",
        }
    }

    /// Whether this tier has a usable (non-fallback) WebGPU adapter.
    #[must_use]
    pub fn has_webgpu(self) -> bool {
        !matches!(self, DeviceTier::WasmOnly)
    }
}

/// Resolve a [`DeviceTier`] from a [`DeviceProbe`].
///
/// Mirrors the shipping `GpuCaps.probe()` logic exactly, then maps its three
/// outcomes (`wasm` / `standard` / `high`) onto the four registry tier keys:
///
/// - no/fallback adapter            → [`DeviceTier::WasmOnly`]
/// - real adapter, not high-headroom → [`DeviceTier::WebgpuLow`] ("standard")
/// - real adapter, high headroom     → [`DeviceTier::WebgpuHigh`] ("high")
///
/// The shipping JS has only `standard`/`high` WebGPU bands; the registry adds a
/// `webgpu_mid` key (used by the Qwen 1.7B default). We map the JS `high` band to
/// [`DeviceTier::WebgpuHigh`] and `standard` to [`DeviceTier::WebgpuLow`]; a
/// device that is WebGPU-capable but below the `high` gate yet has the mid
/// headroom (≥2 GB buffer, ≥8 cores) lands at [`DeviceTier::WebgpuMid`]. This is
/// a strict refinement: the high gate is unchanged, so Voxtral's `webgpu_high`
/// availability is byte-for-byte what the JS computed.
#[must_use]
pub fn resolve_tier(probe: &DeviceProbe) -> DeviceTier {
    if !probe.webgpu_available {
        return DeviceTier::WasmOnly;
    }
    // The shipping high gate (GpuCaps): ≥2 GB max GPU buffer, ≥8 cores, and
    // (unknown memory OR ≥8 GB).
    let mem_ok = probe.memory_gb.is_none_or(|gb| gb >= 8);
    let high = probe.max_gpu_buffer_gb >= 2.0 && probe.thread_count >= 8 && mem_ok;
    if high {
        return DeviceTier::WebgpuHigh;
    }
    // Mid: WebGPU-capable with moderate headroom but below the high gate.
    let mid = probe.max_gpu_buffer_gb >= 2.0 && probe.thread_count >= 8;
    if mid {
        DeviceTier::WebgpuMid
    } else {
        DeviceTier::WebgpuLow
    }
}

/// One option in the settings ASR model picker, sourced from a registry entry's
/// `ui` block (Appendix A row 7). The picker renders these verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickerOption {
    /// The persisted selection key (`localStorage` `settings.model`): `voxtral`,
    /// `nemotron`, `dual`, `sensevoice`, or an HF repo id.
    pub value: String,

    /// The option label, shown verbatim (pixel-identical to the shipping list).
    pub label: String,

    /// The registry id of the underlying model entry (for availability lookup,
    /// precision/backend data, etc.).
    pub model_id: ModelId,

    /// The execution provider the registry records for this engine (`cpu` /
    /// `webgpu`) — drives the row-8 backend default.
    pub backend: String,

    /// The dtype variants the registry records (`fp32`, `q4f16`, …) — drives the
    /// row-8 precision options.
    pub precision: Vec<String>,

    /// Whether this engine is available on the probed device, and if not, why
    /// (R1: reason + recommended CPU-tier alternative).
    pub availability: Availability,
}

/// Whether an engine is available on the probed device — and, when not, the
/// reason plus a recommended CPU-tier alternative (PRD R1 acceptance).
///
/// Never a silent fallback: an unavailable engine is shown in the picker with
/// this verdict so the user sees *why* and what to pick instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Availability {
    /// The engine runs on this device.
    Available,

    /// The engine cannot run on this device.
    Unavailable {
        /// Human-readable reason (e.g. "requires WebGPU, which is not available
        /// on this device").
        reason: String,
        /// The picker `value` of a recommended CPU-tier engine that *will* run
        /// here (`nemotron` — the all-browsers CPU streaming engine), when one
        /// exists.
        recommended: Option<String>,
    },
}

impl Availability {
    /// Whether the engine is available.
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }
}

/// The picker `value` of the recommended CPU-tier ASR engine — the all-browsers
/// streaming default (Nemotron) that runs without WebGPU (PRD R3: "Nemotron
/// 0.6B streaming (CPU, all browsers)"). Used as the `recommended` alternative
/// when a WebGPU engine is unavailable.
const CPU_TIER_RECOMMENDATION: &str = "nemotron";

/// Decide an engine's availability on the probed device.
///
/// The only hard requirement an ASR engine can fail on the client is WebGPU: a
/// `webgpu`-execution engine (Voxtral) is unavailable without a real adapter, and
/// Voxtral additionally wants ≥16 GB (its `webgpu_high` `min_memory_gb`). CPU
/// engines are always available. The verdict carries the reason and the CPU-tier
/// recommendation (R1).
#[must_use]
fn engine_availability(model: &Model, probe: &DeviceProbe) -> Availability {
    // CPU-execution engines run everywhere.
    if model.execution_provider == silent_core::registry::ExecutionProvider::Cpu {
        return Availability::Available;
    }

    // WebGPU-execution engine: needs a real adapter.
    if !probe.webgpu_available {
        return Availability::Unavailable {
            reason: "requires WebGPU, which is not available on this device \
                     (no hardware adapter detected)"
                .to_owned(),
            recommended: Some(CPU_TIER_RECOMMENDATION.to_owned()),
        };
    }

    // WebGPU present — check the engine's own `webgpu_high` memory floor (Voxtral
    // needs 16 GB). The floor lives in the registry `device_tiers` data, so this
    // stays registry-driven (zero code per added model).
    //
    // Memory unknown (None) → assume sufficient, exactly as the tier resolver and
    // the shipping `GpuCaps` do (and as the shipping app does — Voxtral loads on
    // any real adapter today; only a *reported* shortfall blocks it).
    if let Some(min_gb) = model
        .device_tiers
        .get("webgpu_high")
        .filter(|h| h.default_for_tier)
        .and_then(|h| h.min_memory_gb)
        && probe.memory_gb.is_some_and(|gb| gb < min_gb)
    {
        return Availability::Unavailable {
            reason: format!("requires at least {min_gb} GB of memory; this device reports less"),
            recommended: Some(CPU_TIER_RECOMMENDATION.to_owned()),
        };
    }

    Availability::Available
}

/// Build the settings ASR picker option list from the registry, with per-engine
/// availability resolved for the probed device (Appendix A rows 7, 8).
///
/// Every registry entry that has a `ui` block AND is an ASR task becomes one
/// option, ordered by `ui.order` (ties fall back to registry order — stable).
/// This is the whole point: the list is **registry-driven**, so it is exactly
/// the shipping row-7 list today and adding a model is a registry entry tomorrow.
///
/// # Errors
///
/// Returns [`SelectionError::RegistryParse`] if the embedded registry is
/// malformed (a build-integrity failure).
pub fn asr_picker_options(probe: &DeviceProbe) -> Result<Vec<PickerOption>, SelectionError> {
    Ok(asr_picker_options_from(registry()?, probe))
}

/// Build the ASR picker option list from a GIVEN registry (not necessarily the
/// embedded one) — the registry-agnostic core of [`asr_picker_options`].
///
/// This is the "registry entry = zero code" proof point: the entire picker is a
/// function of the registry data. A test can pass a registry with an extra
/// Whisper entry and see it appear in the picker with no code change (see the
/// `adding_a_model_is_zero_code` test). The shipping path always passes the
/// embedded registry via [`asr_picker_options`].
#[must_use]
pub fn asr_picker_options_from(reg: &Registry, probe: &DeviceProbe) -> Vec<PickerOption> {
    let mut options: Vec<(u32, PickerOption)> = reg
        .models
        .iter()
        .filter(|m| m.task == Task::Asr)
        .filter_map(|m| {
            m.ui.as_ref().map(|ui| {
                let availability = engine_availability(m, probe);
                (
                    ui.order,
                    PickerOption {
                        value: ui.value.clone(),
                        label: ui.label.clone(),
                        model_id: m.id.clone(),
                        backend: backend_str(m.execution_provider),
                        precision: m.precision.clone(),
                        availability,
                    },
                )
            })
        })
        .collect();

    options.sort_by_key(|(order, _)| *order);
    options.into_iter().map(|(_, opt)| opt).collect()
}

/// The row-8 backend string for an execution provider (`wasm` / `webgpu`).
///
/// The registry records `cpu` / `webgpu`; the shipping backend picker uses
/// `wasm` / `webgpu` (CPU executes via WASM in the browser). This is the single
/// translation point.
fn backend_str(ep: silent_core::registry::ExecutionProvider) -> String {
    match ep {
        silent_core::registry::ExecutionProvider::Webgpu => "webgpu".to_owned(),
        // CPU and any future additive provider map to the universal WASM backend
        // (`ExecutionProvider` is `#[non_exhaustive]`; the safe default is the
        // backend that runs everywhere).
        _ => "wasm".to_owned(),
    }
}

/// Look up a picker option by its persisted `value` (resolving the registry
/// entry behind a stored `settings.model`), with availability for the probe.
///
/// # Errors
///
/// Returns [`SelectionError::UnknownSelection`] if no registry `ui.value`
/// matches (a stale/unknown stored key), or [`SelectionError::RegistryParse`]
/// for a malformed embedded registry.
pub fn resolve_selection(value: &str, probe: &DeviceProbe) -> Result<PickerOption, SelectionError> {
    asr_picker_options(probe)?
        .into_iter()
        .find(|o| o.value == value)
        .ok_or_else(|| SelectionError::UnknownSelection(value.to_owned()))
}

/// The per-tier default engine recommendation: the registry `ui` ASR engine
/// whose `device_tiers[tier].default_for_tier` is `true` and that is available on
/// the probe. Falls back to the CPU-tier recommendation when no tier default is
/// marked (so there is always a recommendation).
///
/// User choice always wins (R3): this is only a *recommendation* the UI may show;
/// it never overrides a persisted choice.
///
/// # Errors
///
/// Returns [`SelectionError::RegistryParse`] for a malformed embedded registry.
pub fn recommended_default(probe: &DeviceProbe) -> Result<Option<String>, SelectionError> {
    let reg = registry()?;
    let tier = resolve_tier(probe);

    // Prefer a registry ASR engine marked default for this tier that is also
    // available on the device.
    let tier_default = reg
        .models
        .iter()
        .filter(|m| m.task == Task::Asr)
        .filter_map(|m| m.ui.as_ref().map(|ui| (m, ui)))
        .find(|(m, _)| {
            m.device_tiers
                .get(tier.key())
                .is_some_and(|t| t.default_for_tier)
                && engine_availability(m, probe).is_available()
        })
        .map(|(_, ui)| ui.value.clone());

    if tier_default.is_some() {
        return Ok(tier_default);
    }

    // No tier default available — recommend the CPU-tier engine if it is in the
    // registry (it always is: Nemotron).
    let cpu = reg
        .models
        .iter()
        .filter(|m| m.task == Task::Asr)
        .filter_map(|m| m.ui.as_ref())
        .find(|ui| ui.value == CPU_TIER_RECOMMENDATION)
        .map(|ui| ui.value.clone());
    Ok(cpu)
}

/// Construct the [`AnyAsrEngine`] for a persisted picker `value`, **seeding the
/// engine config from registry data** (PRD Phase 5, Task I3; the i2 constants now
/// seed from the registry rather than being chosen by a hand-written JS branch).
///
/// The registry entry decides the engine shape:
///
/// - `composite_of` non-empty → a Dual engine ([`JsHostEngine::dual`]) — the
///   Moonshine [`WhisperStreamConfig::MOONSHINE_DUAL`] draft leg + the SenseVoice
///   refine leg, composed by [`crate::engine::DualMode`].
/// - `host == js-sherpa` (SenseVoice solo) → modeled as a Whisper-stream solo
///   engine over the sherpa host (the policy shape the silent-web trait impl
///   drives); CPU, so `requires_webgpu` is seeded `false` from the registry.
/// - `host == js-transformers` (Voxtral / Whisper / Moonshine) → Voxtral when the
///   registry `execution_provider == webgpu` AND the entry is the Voxtral id;
///   otherwise a Whisper-family solo engine ([`WhisperStreamConfig::WHISPER_SOLO`])
///   whose `requires_webgpu` is **seeded from the registry**
///   `execution_provider` (Whisper large-turbo = webgpu; the small/base/tiny =
///   cpu) — exactly the i2-constant seeding this task moves into the registry.
/// - `host == rust-ort-web` (Nemotron) → [`AnyAsrEngine::Unset`] here: Nemotron's
///   concrete variant is owned by `nemotron-asr` (added there, not in this
///   crate's enum), so the caller routes Nemotron through its own engine. This
///   function returns `Unset` for it rather than mis-modeling it as a `JsHost`.
///
/// # Errors
///
/// Returns [`SelectionError::UnknownSelection`] if `value` matches no registry
/// `ui.value`, or [`SelectionError::RegistryParse`] for a malformed registry.
pub fn engine_for(value: &str) -> Result<AnyAsrEngine, SelectionError> {
    let reg = registry()?;
    let model = reg
        .models
        .iter()
        .find(|m| m.task == Task::Asr && m.ui.as_ref().is_some_and(|ui| ui.value == value))
        .ok_or_else(|| SelectionError::UnknownSelection(value.to_owned()))?;

    let id = model.id.clone();

    // Dual composite (Moonshine drafts + SenseVoice refiner) — config seeded by
    // the composite marker in the registry.
    if !model.composite_of.is_empty() {
        return Ok(JsHostEngine::dual(id).into());
    }

    match model.host {
        Host::JsSherpa => {
            // SenseVoice solo: a final-only Whisper-stream-shaped policy over the
            // sherpa host. CPU → requires_webgpu seeded false from the registry.
            let requires_webgpu =
                model.execution_provider == silent_core::registry::ExecutionProvider::Webgpu;
            Ok(
                JsHostEngine::whisper(id, requires_webgpu, WhisperStreamConfig::WHISPER_SOLO)
                    .into(),
            )
        }
        Host::JsTransformers => {
            // Voxtral is the one js-transformers engine with the two-cap recycle
            // policy; identify it by its WebGPU-required, recycle-shaped entry.
            if value == "voxtral" {
                return Ok(JsHostEngine::voxtral(id, RecycleConfig::default()).into());
            }
            // Whisper family + Moonshine solo: WHISPER_SOLO chunking; the
            // WebGPU requirement is SEEDED FROM THE REGISTRY execution_provider
            // (the i2 constant that used to be a JS branch).
            let requires_webgpu =
                model.execution_provider == silent_core::registry::ExecutionProvider::Webgpu;
            Ok(
                JsHostEngine::whisper(id, requires_webgpu, WhisperStreamConfig::WHISPER_SOLO)
                    .into(),
            )
        }
        // Nemotron (rust-ort-web): its concrete AnyAsrEngine variant is owned by
        // nemotron-asr, not this crate's enum. Return Unset so the caller routes
        // it through the Nemotron engine rather than mis-modeling it here.
        _ => Ok(AnyAsrEngine::Unset),
    }
}

/// The queued mid-recording switch policy (PRD R3 decision log: "Mid-recording
/// engine switch queues for next meeting").
///
/// When the user changes the ASR engine, this decides what happens:
///
/// - **Not recording** → [`SwitchOutcome::AppliedNow`]: the new selection takes
///   effect at the next recording start (the normal path).
/// - **Recording** → [`SwitchOutcome::QueuedForNextMeeting`]: the change is
///   *accepted and queued* — never silently dropped, never hard-rejected — with a
///   friendly "takes effect next meeting" notice the UI surfaces.
///
/// The selection is always persisted by the caller regardless of outcome (the
/// user's choice wins); this only decides *when* it takes effect and what notice
/// to show.
#[must_use]
pub fn apply_selection(new_value: &str, is_recording: bool) -> SwitchOutcome {
    if is_recording {
        SwitchOutcome::QueuedForNextMeeting {
            value: new_value.to_owned(),
            notice: "Switched to a new transcription engine — it takes effect for your \
                 next meeting. This recording continues on the current engine."
                .to_owned(),
        }
    } else {
        SwitchOutcome::AppliedNow {
            value: new_value.to_owned(),
        }
    }
}

/// The outcome of an engine-selection change — a typed notice event, never a
/// silent failure or hard rejection (PRD R3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SwitchOutcome {
    /// Not recording: the selection applies at the next recording start.
    AppliedNow {
        /// The selected picker `value`.
        value: String,
    },
    /// Recording: the change is queued for the next meeting with a friendly
    /// notice. The current recording keeps its engine.
    QueuedForNextMeeting {
        /// The selected picker `value` (queued).
        value: String,
        /// The friendly notice the UI shows ("takes effect next meeting").
        notice: String,
    },
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the workspace \
              lint config permits this in test code (PRD 'Rust engineering bar')"
)]
mod tests {
    use super::*;

    /// The exact row-7 shipping list (value, label) in order — the contract this
    /// module must reproduce from the registry. Mirrors `index.html`'s
    /// `MODEL_OPTIONS` (the shipping app's picker, lines ~6656-6666).
    const SHIPPING_ROW7: &[(&str, &str)] = &[
        ("voxtral", "Voxtral Realtime 4B (WebGPU, ~2.7GB, streaming)"),
        (
            "nemotron",
            "Nemotron Streaming 0.6B (WASM/CPU, ~917MB, streaming · frees GPU for AI notes)",
        ),
        ("dual", "Dual: Moonshine (instant) + SenseVoice (refined)"),
        ("sensevoice", "SenseVoice only (~253MB, no 30s window)"),
        (
            "onnx-community/whisper-large-v3-turbo",
            "Whisper Large v3 Turbo (~563MB, WebGPU)",
        ),
        (
            "onnx-community/whisper-small.en",
            "Whisper Small EN (good accuracy, ~500MB)",
        ),
        ("onnx-community/whisper-base.en", "Whisper Base EN (~200MB)"),
        (
            "onnx-community/moonshine-base-ONNX",
            "Moonshine Base (fastest, ~150MB)",
        ),
        ("onnx-community/whisper-tiny.en", "Whisper Tiny EN (~100MB)"),
    ];

    fn webgpu_high_probe() -> DeviceProbe {
        DeviceProbe {
            webgpu_available: true,
            memory_gb: Some(16),
            cross_origin_isolated: true,
            thread_count: 12,
            max_gpu_buffer_gb: 4.0,
        }
    }

    /// R3 acceptance: "Adding a model within an existing family (another Whisper
    /// size) is a registry entry — zero code." We prove it by appending a
    /// TEST-ONLY Whisper entry to the embedded registry TOML, parsing the result,
    /// and showing the SAME `asr_picker_options_from` picks it up — no new code,
    /// no new branch. The fixture entry lives ONLY in this test string; it is
    /// never shipped (the embedded `registry/models.toml` is unchanged).
    #[test]
    fn adding_a_model_is_zero_code() {
        // A new Whisper size — the kind of entry R3 says must be zero-code.
        const FIXTURE_ENTRY: &str = r#"

[[model]]
id = "asr.whisper.medium_en_TEST_FIXTURE"
task = "asr"
provider = "huggingface"
repo = "onnx-community/whisper-medium.en"
revision = "0000000000000000000000000000000000000000"
host = "js-transformers"
execution_provider = "cpu"
precision = ["int8", "fp16"]
memory_budget_mb = 900
license = "apache-2.0"
license_verified = false
network_origins = ["https://huggingface.co"]

  [model.cache]
  store = "transformers-idb"

  [model.ui]
  value = "onnx-community/whisper-medium.en"
  label = "Whisper Medium EN (TEST FIXTURE — not shipped)"
  order = 10
"#;
        let toml_with_fixture = format!("{EMBEDDED_REGISTRY_TOML}{FIXTURE_ENTRY}");
        let reg: Registry =
            toml::from_str(&toml_with_fixture).expect("registry + fixture entry must parse");

        let opts = asr_picker_options_from(&reg, &webgpu_high_probe());

        // The fixture appears in the picker — purely from the registry data.
        let fixture = opts
            .iter()
            .find(|o| o.value == "onnx-community/whisper-medium.en")
            .expect("the test-only Whisper entry must appear in the picker — zero code");
        assert_eq!(
            fixture.label,
            "Whisper Medium EN (TEST FIXTURE — not shipped)"
        );
        assert_eq!(fixture.backend, "wasm"); // cpu → wasm, from the registry
        assert_eq!(
            fixture.precision,
            vec!["int8".to_owned(), "fp16".to_owned()]
        );
        assert!(fixture.availability.is_available());
        // It slots into the order: order=10 places it last (after tiny.en=9).
        assert_eq!(
            opts.last().map(|o| o.value.as_str()),
            Some("onnx-community/whisper-medium.en"),
            "order=10 places the fixture last"
        );

        // And the SHIPPING embedded registry does NOT contain the fixture (we did
        // not ship it).
        let shipped = asr_picker_options(&webgpu_high_probe()).unwrap();
        assert!(
            !shipped
                .iter()
                .any(|o| o.value == "onnx-community/whisper-medium.en"),
            "the fixture entry must NOT be in the shipped registry"
        );
    }

    #[test]
    fn embedded_registry_parses() {
        let reg = registry().expect("embedded registry must parse");
        assert!(
            reg.models.len() >= 12,
            "expected >=12 models (11 originals + Dual), got {}",
            reg.models.len()
        );
    }

    #[test]
    fn picker_list_matches_shipping_row7_exactly() {
        // On a fully-capable device every engine is available, so the list is the
        // shipping row-7 list, value-for-value and label-for-label, in order.
        let opts = asr_picker_options(&webgpu_high_probe()).expect("picker options");
        let got: Vec<(&str, &str)> = opts
            .iter()
            .map(|o| (o.value.as_str(), o.label.as_str()))
            .collect();
        let want: Vec<(&str, &str)> = SHIPPING_ROW7.to_vec();
        assert_eq!(
            got, want,
            "registry-driven picker must reproduce the shipping row-7 list exactly"
        );
    }

    #[test]
    fn no_webgpu_marks_voxtral_unavailable_with_reason_and_recommends_nemotron() {
        // Simulate the witness probe: no WebGPU.
        let probe = DeviceProbe::cpu_only();
        let opts = asr_picker_options(&probe).expect("picker options");
        let voxtral = opts
            .iter()
            .find(|o| o.value == "voxtral")
            .expect("voxtral option present");
        match &voxtral.availability {
            Availability::Unavailable {
                reason,
                recommended,
            } => {
                assert!(
                    reason.to_lowercase().contains("webgpu"),
                    "reason must mention WebGPU, got {reason:?}"
                );
                assert_eq!(
                    recommended.as_deref(),
                    Some("nemotron"),
                    "must recommend the CPU-tier engine (Nemotron)"
                );
            }
            Availability::Available => panic!("Voxtral must be unavailable without WebGPU"),
        }
        // Nemotron itself stays available on CPU.
        let nemo = opts
            .iter()
            .find(|o| o.value == "nemotron")
            .expect("nemotron option present");
        assert!(
            nemo.availability.is_available(),
            "Nemotron (CPU) must be available without WebGPU"
        );
    }

    #[test]
    fn cpu_engines_available_everywhere() {
        let probe = DeviceProbe::cpu_only();
        let opts = asr_picker_options(&probe).expect("picker options");
        for value in ["nemotron", "sensevoice", "dual"] {
            let o = opts.iter().find(|o| o.value == value).unwrap();
            assert!(
                o.availability.is_available(),
                "{value} (CPU host) must be available without WebGPU"
            );
        }
    }

    #[test]
    fn row8_backend_and_precision_come_from_registry() {
        let opts = asr_picker_options(&webgpu_high_probe()).expect("picker options");
        let voxtral = opts.iter().find(|o| o.value == "voxtral").unwrap();
        assert_eq!(voxtral.backend, "webgpu");
        assert_eq!(voxtral.precision, vec!["q4f16".to_owned()]);

        let nemo = opts.iter().find(|o| o.value == "nemotron").unwrap();
        assert_eq!(nemo.backend, "wasm"); // cpu → wasm
        assert_eq!(nemo.precision, vec!["int8".to_owned()]);
    }

    #[test]
    fn tier_resolution_mirrors_gpu_caps() {
        // No adapter → wasm_only.
        assert_eq!(resolve_tier(&DeviceProbe::cpu_only()), DeviceTier::WasmOnly);
        // High gate met → webgpu_high.
        assert_eq!(resolve_tier(&webgpu_high_probe()), DeviceTier::WebgpuHigh);
        // Real adapter, weak (small buffer, few cores) → webgpu_low.
        let low = DeviceProbe {
            webgpu_available: true,
            memory_gb: Some(8),
            cross_origin_isolated: true,
            thread_count: 4,
            max_gpu_buffer_gb: 1.0,
        };
        assert_eq!(resolve_tier(&low), DeviceTier::WebgpuLow);
        // Mid: big buffer + cores but the JS-`high` band is the same gate, so a
        // device passing both buffer+cores resolves high; to land at mid we need
        // one gate met and not the strict high (e.g. cores≥8, buffer≥2 but
        // memory reported <8 fails high's mem_ok → mid).
        let mid = DeviceProbe {
            webgpu_available: true,
            memory_gb: Some(4),
            cross_origin_isolated: true,
            thread_count: 8,
            max_gpu_buffer_gb: 2.0,
        };
        assert_eq!(resolve_tier(&mid), DeviceTier::WebgpuMid);
    }

    #[test]
    fn recommended_default_follows_tier() {
        // High tier → Voxtral is the webgpu_high default and available.
        assert_eq!(
            recommended_default(&webgpu_high_probe())
                .unwrap()
                .as_deref(),
            Some("voxtral")
        );
        // CPU tier → Nemotron (wasm_only default).
        assert_eq!(
            recommended_default(&DeviceProbe::cpu_only())
                .unwrap()
                .as_deref(),
            Some("nemotron")
        );
    }

    #[test]
    fn mid_recording_switch_queues_with_friendly_notice() {
        // Recording → queued, never rejected, never silent.
        let out = apply_selection("whisper-tiny", true);
        match out {
            SwitchOutcome::QueuedForNextMeeting { value, notice } => {
                assert_eq!(value, "whisper-tiny");
                assert!(
                    notice.to_lowercase().contains("next meeting"),
                    "notice must say it takes effect next meeting, got {notice:?}"
                );
            }
            SwitchOutcome::AppliedNow { .. } => {
                panic!("a mid-recording switch must queue, not apply now")
            }
        }
    }

    #[test]
    fn switch_while_idle_applies_now() {
        let out = apply_selection("voxtral", false);
        assert_eq!(
            out,
            SwitchOutcome::AppliedNow {
                value: "voxtral".to_owned()
            }
        );
    }

    #[test]
    fn engine_for_seeds_requires_webgpu_from_registry() {
        use crate::engine::JsHostPolicy;

        // Whisper large-turbo: registry execution_provider = webgpu → the engine's
        // requires_webgpu is SEEDED true from the registry (the i2 constant).
        let turbo = engine_for("onnx-community/whisper-large-v3-turbo").unwrap();
        assert!(
            turbo.capabilities().unwrap().requires_webgpu,
            "Whisper large-turbo must seed requires_webgpu=true from the registry webgpu provider"
        );

        // Whisper tiny: registry execution_provider = cpu → requires_webgpu false.
        let tiny = engine_for("onnx-community/whisper-tiny.en").unwrap();
        assert!(
            !tiny.capabilities().unwrap().requires_webgpu,
            "Whisper tiny must seed requires_webgpu=false from the registry cpu provider"
        );

        // Voxtral → the two-cap recycle policy, requires_webgpu true.
        let vox = engine_for("voxtral").unwrap();
        assert!(vox.capabilities().unwrap().requires_webgpu);
        if let AnyAsrEngine::JsHost(e) = &vox {
            assert!(matches!(e.policy(), JsHostPolicy::Voxtral(_)));
        } else {
            panic!("voxtral must be a JsHost engine");
        }

        // Dual composite → the Dual policy with both legs, declares drafts.
        let dual = engine_for("dual").unwrap();
        assert!(dual.capabilities().unwrap().drafts, "Dual declares drafts");
        if let AnyAsrEngine::JsHost(e) = &dual {
            assert!(matches!(e.policy(), JsHostPolicy::Dual(_)));
        } else {
            panic!("dual must be a JsHost engine");
        }

        // Nemotron → Unset (its concrete variant lives in nemotron-asr).
        let nemo = engine_for("nemotron").unwrap();
        assert!(
            !nemo.is_set(),
            "Nemotron routes through its own engine; engine_for returns Unset"
        );

        // Unknown value → loud error, never a silent default.
        assert_eq!(
            engine_for("nope").unwrap_err(),
            SelectionError::UnknownSelection("nope".to_owned())
        );
    }

    #[test]
    fn resolve_selection_finds_known_value_and_rejects_unknown() {
        let probe = webgpu_high_probe();
        let opt = resolve_selection("dual", &probe).expect("dual resolves");
        assert_eq!(opt.model_id, ModelId::new("asr.dual.moonshine_sensevoice"));

        let err = resolve_selection("does-not-exist", &probe).unwrap_err();
        assert_eq!(
            err,
            SelectionError::UnknownSelection("does-not-exist".to_owned())
        );
    }
}
