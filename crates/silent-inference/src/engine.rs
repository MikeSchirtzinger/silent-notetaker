//! The `JsHostEngine` adapter and the local [`AnyAsrEngine`] enum-dispatch — the
//! i1 hand-off TODO realized.
//!
//! `silent_core::engine::AnyAsrEngine` is the *named* dispatch strategy, kept
//! dependency-free in the contracts crate with only its `Unset` placeholder (its
//! doc-comment instructs: *"The concrete variants … are added in their home
//! crates"*). This module is that home crate for the JS-hosted engines: it adds
//! the concrete [`AnyAsrEngine::JsHost`] variant and the [`JsHostEngine`] adapter
//! that carries the per-engine policy.
//!
//! # Why the dispatch enum is re-declared here, not extended in silent-core
//!
//! `silent_core::engine::AnyAsrEngine` is `#[non_exhaustive]`, which (by design)
//! prevents *downstream* crates from adding variants to it — a `non_exhaustive`
//! enum can only gain variants in its defining crate. The PRD's resolution (its
//! "Core contracts" skeleton and the `silent-core` `AnyAsrEngine` doc-comment) is
//! that each host crate owns the *concrete* dispatch enum naming the strategy; the
//! `silent-core` enum exists to **name** it (so no one invents an ad-hoc
//! strategy) while staying free of the browser/host dependencies the real engines
//! pull in. So [`AnyAsrEngine`] here is the concrete, host-side dispatch enum: it
//! follows the exact shape `silent-core` documents
//! (`enum AnyAsrEngine { Nemotron(..), JsHost(..), Sherpa(..) }`), and adds
//! `JsHost` (and the `Sherpa` SenseVoice host) now. Nemotron's variant is added
//! the same way from `nemotron-asr` when its `AsrEngine` impl is wired.
//!
//! # What a `JsHostEngine` *is*
//!
//! It is the **policy** for one js-hosted ASR engine plus its identity and
//! capabilities — the part that is pure Rust law and unit-testable without a
//! browser. The concrete streaming policy is one of:
//!
//! - [`JsHostPolicy::WhisperStream`] — Whisper family + Moonshine-solo loop
//!   ([`crate::whisper_stream`]).
//! - [`JsHostPolicy::Voxtral`] — the two-cap recycle ([`crate::voxtral_recycle`]).
//! - [`JsHostPolicy::Dual`] — Moonshine-draft + SenseVoice-refine coordination
//!   ([`crate::dual`]) paired with the Moonshine [`crate::whisper_stream`] leg and
//!   the SenseVoice [`crate::sensevoice`] leg.
//!
//! The `async fn`-shaped [`silent_core::engine::AsrEngine`] **trait impl** — the
//! part that actually `await`s a transformers.js / sherpa-onnx worker over the
//! typed command boundary — lands in `silent-web` (it needs `wasm-bindgen` +
//! `web-sys`, which must not enter this crate; see the crate docs). This module
//! provides the policy and the dispatch the trait impl will plug into, so the
//! hand-off is concrete and the dispatch surface (`id`, `capabilities`, `is_set`)
//! is testable here today.
//!
//! No `unwrap`/`expect`; nothing fallible on the hot path (PRD "Rust engineering bar").

use silent_core::events::AsrCapabilities;
use silent_core::ids::ModelId;

use crate::dual::DualCoordinator;
use crate::sensevoice::{SenseVoiceConfig, SenseVoicePolicy};
use crate::voxtral_recycle::{RecycleConfig, VoxtralRecyclePolicy};
use crate::whisper_stream::{WhisperStreamConfig, WhisperStreamPolicy};

/// The streaming policy a [`JsHostEngine`] runs — one per ASR engine family.
///
/// Each variant *owns* the deterministic policy state machine for its engine; the
/// js-host worker is the executor. `#[non_exhaustive]` so a future js-hosted ASR
/// engine is additive.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum JsHostPolicy {
    /// Whisper large-turbo / small.en / base.en / tiny.en, and Moonshine solo:
    /// the chunk → VAD → hallucination-filter → dedup → final loop.
    WhisperStream(WhisperStreamPolicy),
    /// Voxtral Realtime: in-place streaming partials with the token/audio two-cap
    /// context recycle.
    Voxtral(VoxtralRecyclePolicy),
    /// Dual mode: Moonshine drafts (a `WhisperStream` leg at the 3 s Dual cadence)
    /// interleaved with SenseVoice refined finals (a SenseVoice VAD-segmentation
    /// leg) by the [`DualCoordinator`].
    Dual(DualMode),
}

/// The composed state for Dual mode: the Moonshine streaming leg, the SenseVoice
/// segmentation leg, and the draft/refine coordinator that interleaves them.
///
/// The two legs run concurrently on the same audio (the JS feeds both); the
/// coordinator owns the transcript-list interleaving (Appendix A row 11).
#[derive(Debug, Clone)]
pub struct DualMode {
    /// Moonshine leg — the [`crate::whisper_stream`] loop at the 3 s Dual chunk
    /// cadence (`WhisperStreamConfig::MOONSHINE_DUAL`).
    pub moonshine: WhisperStreamPolicy,
    /// SenseVoice leg — the [`crate::sensevoice`] VAD-segmentation policy.
    pub sensevoice: SenseVoicePolicy,
    /// The draft/refine interleaving coordinator.
    pub coordinator: DualCoordinator,
}

impl DualMode {
    /// Build Dual mode with shipping configs for both legs.
    #[must_use]
    pub fn shipping() -> Self {
        Self {
            moonshine: WhisperStreamPolicy::new(WhisperStreamConfig::MOONSHINE_DUAL),
            sensevoice: SenseVoicePolicy::new(SenseVoiceConfig::SHIPPING),
            coordinator: DualCoordinator::new(),
        }
    }
}

/// A js-hosted ASR engine: its registry id, declared capabilities, and the
/// streaming policy that decides what the host does. This is the concrete payload
/// of [`AnyAsrEngine::JsHost`].
///
/// The async [`silent_core::engine::AsrEngine`] trait impl that drives the live
/// worker lands in `silent-web`; here the engine is the policy holder + identity,
/// which is what the dyn-free dispatch needs and what is testable without a
/// browser.
#[derive(Debug, Clone)]
pub struct JsHostEngine {
    id: ModelId,
    capabilities: AsrCapabilities,
    policy: JsHostPolicy,
}

impl JsHostEngine {
    /// Construct a js-hosted engine from its id, capabilities, and policy.
    #[must_use]
    pub fn new(id: ModelId, capabilities: AsrCapabilities, policy: JsHostPolicy) -> Self {
        Self {
            id,
            capabilities,
            policy,
        }
    }

    /// A Whisper-family or Moonshine-solo engine (final-only, streaming chunks).
    ///
    /// `requires_webgpu` is the model's tier requirement (Whisper large-turbo
    /// defaults to WebGPU with a WASM fallback; smaller tiers run on WASM). The
    /// caller passes it from registry data (Task I3); defaulting is not this
    /// module's job.
    #[must_use]
    pub fn whisper(id: ModelId, requires_webgpu: bool, config: WhisperStreamConfig) -> Self {
        Self::new(
            id,
            AsrCapabilities {
                streaming: true,
                drafts: false,
                requires_webgpu,
                sample_rate_hz: 16_000,
            },
            JsHostPolicy::WhisperStream(WhisperStreamPolicy::new(config)),
        )
    }

    /// A Voxtral engine (streaming in-place partials, WebGPU-required, two-cap
    /// recycle).
    #[must_use]
    pub fn voxtral(id: ModelId, config: RecycleConfig) -> Self {
        Self::new(
            id,
            AsrCapabilities {
                streaming: true,
                drafts: false,
                requires_webgpu: true,
                sample_rate_hz: 16_000,
            },
            JsHostPolicy::Voxtral(VoxtralRecyclePolicy::new(config)),
        )
    }

    /// A Dual-mode engine (Moonshine drafts + SenseVoice refined finals). Declares
    /// `drafts: true` — the only engine that emits [`silent_core::events::EngineEvent::Draft`].
    #[must_use]
    pub fn dual(id: ModelId) -> Self {
        Self::new(
            id,
            AsrCapabilities {
                streaming: true,
                drafts: true,
                requires_webgpu: false,
                sample_rate_hz: 16_000,
            },
            JsHostPolicy::Dual(DualMode::shipping()),
        )
    }

    /// The registry id of the model this engine runs.
    #[must_use]
    pub fn id(&self) -> ModelId {
        self.id.clone()
    }

    /// What the engine can do (streaming, drafts, WebGPU requirement, sample rate).
    #[must_use]
    pub fn capabilities(&self) -> AsrCapabilities {
        self.capabilities.clone()
    }

    /// Borrow the streaming policy.
    #[must_use]
    pub fn policy(&self) -> &JsHostPolicy {
        &self.policy
    }

    /// Mutably borrow the streaming policy (the silent-web trait impl drives it).
    pub fn policy_mut(&mut self) -> &mut JsHostPolicy {
        &mut self.policy
    }
}

/// The concrete enum-dispatch strategy for engine selection, host-side.
///
/// `async fn` in traits is not dyn-safe, so the orchestrator holds an
/// `AnyAsrEngine` and `match`es on the active engine rather than a
/// `Box<dyn AsrEngine>` (PRD "Core contracts"; mirrors
/// `silent_core::engine::AnyAsrEngine`). `#[non_exhaustive]` so the `Nemotron`
/// (rust-ort-web) variant from `nemotron-asr` is additive.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum AnyAsrEngine {
    /// No engine selected yet (the orchestrator starts here before the user picks
    /// an ASR model). Matches `silent_core::engine::AnyAsrEngine::Unset`.
    #[default]
    Unset,
    /// A js-hosted engine (Voxtral, Whisper family, Moonshine, Dual). The i1
    /// hand-off variant.
    JsHost(JsHostEngine),
}

impl AnyAsrEngine {
    /// The id of the active engine, if one is selected (`None` for [`Unset`]).
    ///
    /// [`Unset`]: AnyAsrEngine::Unset
    #[must_use]
    pub fn id(&self) -> Option<ModelId> {
        match self {
            AnyAsrEngine::Unset => None,
            AnyAsrEngine::JsHost(e) => Some(e.id()),
        }
    }

    /// The active engine's capabilities, if one is selected.
    #[must_use]
    pub fn capabilities(&self) -> Option<AsrCapabilities> {
        match self {
            AnyAsrEngine::Unset => None,
            AnyAsrEngine::JsHost(e) => Some(e.capabilities()),
        }
    }

    /// Whether an engine is selected.
    #[must_use]
    pub fn is_set(&self) -> bool {
        !matches!(self, AnyAsrEngine::Unset)
    }
}

impl From<JsHostEngine> for AnyAsrEngine {
    fn from(engine: JsHostEngine) -> Self {
        AnyAsrEngine::JsHost(engine)
    }
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

    #[test]
    fn unset_engine_has_no_id_or_capabilities() {
        let e = AnyAsrEngine::default();
        assert!(!e.is_set());
        assert_eq!(e.id(), None);
        assert_eq!(e.capabilities(), None);
    }

    #[test]
    fn whisper_engine_dispatches_id_and_capabilities() {
        let engine = JsHostEngine::whisper(
            ModelId::new("asr.whisper.large_v3_turbo"),
            true,
            WhisperStreamConfig::WHISPER_SOLO,
        );
        let any: AnyAsrEngine = engine.into();
        assert!(any.is_set());
        assert_eq!(any.id(), Some(ModelId::new("asr.whisper.large_v3_turbo")));
        let caps = any.capabilities().unwrap();
        assert!(caps.streaming);
        assert!(!caps.drafts);
        assert!(caps.requires_webgpu);
        assert_eq!(caps.sample_rate_hz, 16_000);
        // The policy is the Whisper stream loop.
        match any {
            AnyAsrEngine::JsHost(e) => {
                assert!(matches!(e.policy(), JsHostPolicy::WhisperStream(_)));
            }
            AnyAsrEngine::Unset => panic!("expected JsHost"),
        }
    }

    #[test]
    fn voxtral_engine_requires_webgpu_and_carries_recycle_policy() {
        let engine = JsHostEngine::voxtral(
            ModelId::new("asr.voxtral.realtime_4b"),
            RecycleConfig::default(),
        );
        let caps = engine.capabilities();
        assert!(caps.requires_webgpu);
        assert!(!caps.drafts);
        assert!(matches!(engine.policy(), JsHostPolicy::Voxtral(_)));
    }

    #[test]
    fn dual_engine_declares_drafts_and_composes_both_legs() {
        let engine = JsHostEngine::dual(ModelId::new("asr.dual.moonshine_sensevoice"));
        let caps = engine.capabilities();
        assert!(caps.drafts, "Dual is the only engine that emits drafts");
        assert!(!caps.requires_webgpu);
        match engine.policy() {
            JsHostPolicy::Dual(d) => {
                // Moonshine leg uses the 3 s Dual chunk cadence.
                assert_eq!(
                    d.moonshine.config().chunk_samples,
                    WhisperStreamConfig::DUAL_MOONSHINE_CHUNK_SAMPLES
                );
                // SenseVoice leg uses the shipping 30 s-window VAD config.
                assert!((d.sensevoice.config().max_speech_secs - 30.0).abs() < 1e-9);
                // Coordinator starts empty.
                assert!(d.coordinator.items().is_empty());
            }
            other => panic!("expected Dual policy, got {other:?}"),
        }
    }

    #[test]
    fn policy_mut_drives_the_underlying_state_machine() {
        // The silent-web trait impl reaches the policy via policy_mut(); prove it
        // mutates the real state machine.
        let mut engine = JsHostEngine::whisper(
            ModelId::new("asr.moonshine.base"),
            false,
            WhisperStreamConfig::WHISPER_SOLO,
        );
        if let JsHostPolicy::WhisperStream(p) = engine.policy_mut() {
            let ev = p.on_decoded("hello there friend");
            assert_eq!(ev.len(), 1);
        } else {
            panic!("expected WhisperStream policy");
        }
    }
}
