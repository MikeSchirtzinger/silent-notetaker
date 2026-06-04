//! Silent Notetaker domain contracts.
//!
//! `silent-core` is the single source of truth for the typed boundary between
//! the (unchanged) browser UI and the Rust core. It contains:
//!
//! - [`commands`] — `UiCommand` (UI → core) and `SessionEvent` (core → UI),
//!   the versioned, `#[non_exhaustive]` boundary message types.
//! - [`events`] — [`events::EngineEvent`] and the engine telemetry types, the
//!   streaming lifecycle the ASR engines emit.
//! - [`error`] — the shared [`error::AsrError`] (one error type across every
//!   engine, so engine swapping needs no per-engine error handling) and
//!   [`error::ModelResolveError`].
//! - [`engine`] — the async-first [`engine::AsrEngine`] trait, the named
//!   [`engine::AnyAsrEngine`] enum-dispatch strategy, and the notes/question
//!   trait shapes.
//! - [`registry`] — the Hugging Face model registry types (PRD R4): the repo
//!   stores typed model metadata, never weights. The registry drives engine
//!   selection, CSP generation, the egress manifest, license display, and cache
//!   verification.
//! - [`ids`] — small shared value types ([`ids::ModelId`], [`ids::TimeRange`]).
//!
//! # No browser dependencies
//!
//! This crate must compile for `wasm32-unknown-unknown` and must be testable
//! without a browser or a GPU. It has no `wasm-bindgen`, `web-sys`, or runtime
//! dependency beyond `serde`/`thiserror`. Browser glue belongs in `silent-web`.
//!
//! # TypeScript boundary
//!
//! The boundary types derive [`ts_rs::TS`] under `#[cfg(test)]` and are exported
//! to `bindings/` by the `export_bindings` test (see `tests/`). The committed
//! bindings are the contract; CI fails if they drift from the Rust types. Per
//! the A3 spike, `#[ts(transparent)]` is applied alongside `#[serde(transparent)]`
//! wherever a newtype is transparent, so ts-rs emits the correct alias.

pub mod commands;
pub mod diarization;
pub mod engine;
pub mod error;
pub mod events;
pub mod ids;
pub mod questions;
pub mod registry;

pub use error::{AsrError, ModelResolveError};
pub use events::{EngineEvent, EngineStats};
pub use ids::{ModelId, TimeRange};

/// Version of the UI↔core boundary contract.
///
/// Bumped when a breaking change is made to [`commands`] or [`events`]. The UI
/// reads this to refuse to run against an incompatible core. Boundary event
/// enums are additionally `#[non_exhaustive]`, so additive variants do not
/// require a major bump — only removals or shape changes do.
pub const BOUNDARY_CONTRACT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// TypeScript binding generation (A3 spike pattern).
//
// The `TS` derive is applied via `#[cfg_attr(test, derive(ts_rs::TS))]`, so it
// is available only in this crate's UNIT-test build (where `cfg(test)` is true).
// That is why generation lives here as a unit test rather than in `tests/`:
// integration tests link the lib compiled WITHOUT `cfg(test)`, where the derive
// would be absent. Run with:
//
//     cargo test -p silent-core export_bindings
//     git diff --exit-code crates/silent-core/bindings/   # CI freshness gate
// ---------------------------------------------------------------------------
#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism; the PRD lint \
              config allows this in tests"
)]
mod ts_bindings {
    use crate::commands::{SessionEvent, SessionState, UiCommand};
    use crate::diarization::{
        DiarizationCommand, DiarizationEvent, RelabelEntry, SpeakerDescriptor,
    };
    use crate::error::{AsrError, ModelResolveError};
    use crate::events::{AsrCapabilities, EngineEvent, EngineStats};
    use crate::ids::{ModelId, TimeRange};
    use crate::questions::{QuestionCommand, QuestionEvent, QuestionType, QwenNote, RecapGroup};
    use crate::registry::{
        Cache, CacheStore, DeviceTier, ExecutionProvider, Host, Model, ModelFile, PerfBudget,
        Provider, Registry, Task, Validation,
    };
    use std::path::Path;
    use ts_rs::TS;

    #[test]
    fn export_bindings() {
        // `export_all` on each top-level type also exports every type it
        // references transitively, writing one `.ts` per type into the crate's
        // `bindings/` dir (`TS_RS_EXPORT_DIR`, default `<crate>/bindings`). The
        // committed output is the boundary contract; CI fails on drift.
        macro_rules! export {
            ($($t:ty),+ $(,)?) => {
                $( <$t as TS>::export_all().expect(concat!("export ", stringify!($t))); )+
            };
        }

        export!(
            ModelId,
            TimeRange,
            EngineEvent,
            EngineStats,
            AsrCapabilities,
            AsrError,
            ModelResolveError,
            UiCommand,
            SessionEvent,
            SessionState,
            DiarizationCommand,
            DiarizationEvent,
            SpeakerDescriptor,
            RelabelEntry,
            QuestionCommand,
            QuestionEvent,
            QuestionType,
            RecapGroup,
            QwenNote,
            Registry,
            Model,
            ModelFile,
            Cache,
            CacheStore,
            DeviceTier,
            Validation,
            PerfBudget,
            Task,
            Provider,
            Host,
            ExecutionProvider,
        );

        let bindings = Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings");
        assert!(
            bindings.is_dir(),
            "bindings dir should exist after export: {}",
            bindings.display()
        );
        for expected in [
            "EngineEvent.ts",
            "UiCommand.ts",
            "SessionEvent.ts",
            "DiarizationCommand.ts",
            "DiarizationEvent.ts",
            "SpeakerDescriptor.ts",
            "RelabelEntry.ts",
            "QuestionCommand.ts",
            "QuestionEvent.ts",
            "QuestionType.ts",
            "RecapGroup.ts",
            "QwenNote.ts",
            "AsrError.ts",
            "Registry.ts",
            "Model.ts",
            "ModelId.ts",
        ] {
            let p = bindings.join(expected);
            assert!(p.is_file(), "expected generated binding {}", p.display());
        }
    }

    /// `ModelId` must generate `type ModelId = string` (transparent), not a
    /// wrapper object — the `#[ts(transparent)]`-alongside-`#[serde(transparent)]`
    /// guarantee from the A3 spike. A regression (ts-rs emitting `{ "0": string }`)
    /// would silently break the UI typing.
    #[test]
    fn model_id_is_a_transparent_string_alias() {
        let decl = <ModelId as TS>::decl();
        assert!(
            decl.contains("= string"),
            "ModelId must be a transparent string alias, got: {decl}"
        );
    }
}
