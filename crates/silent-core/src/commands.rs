//! The versioned UI↔core boundary message types.
//!
//! [`UiCommand`] flows UI → core (the "hands" telling the "law" what the user
//! did); [`SessionEvent`] flows core → UI (the law telling the UI what to
//! render). Both are `#[non_exhaustive]` (PRD "The UI boundary"): additive
//! variants do not break the UI, which handles unknown variants with a wildcard
//! `switch` arm (A3 escape-hatch). TypeScript definitions are generated from
//! these types; a boundary change that would break the UI fails at build time.
//!
//! **Command-log replay** (PRD "The UI boundary"): the [`UiCommand`] stream is
//! capturable and replayable deterministically — the difference between "cannot
//! reproduce" and a failing test for a long-running streaming audio app. These
//! types are `serde` round-trippable to support exactly that.

use serde::{Deserialize, Serialize};

use crate::error::AsrError;
use crate::events::EngineEvent;
use crate::ids::ModelId;

/// A command issued by the UI to the core.
///
/// Tagged `{ "tag": "...", "payload": ... }`, matching the [`SessionEvent`] and
/// [`EngineEvent`] discriminant layout the A3 spike validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UiCommand {
    /// Start a new recording with the given title.
    StartRecording {
        /// Meeting title (auto date/time default, up to 120 chars per
        /// Appendix A row 3).
        title: String,
    },

    /// Stop the active recording (triggers finalize + stop-time notes/recap).
    StopRecording,

    /// Resume/continue the previous recording without reloading the model
    /// (Appendix A row 2).
    ResumeRecording,

    /// Reset to a fresh meeting (Appendix A row 3).
    NewMeeting,

    /// Select the active ASR engine by registry id. Applies at the next
    /// recording start; a mid-recording change is queued with a friendly notice
    /// (PRD R3 / Appendix C).
    SelectAsrEngine {
        /// Registry id of the chosen ASR model.
        model: ModelId,
    },

    /// Select the notes/questions model, or `None` for transcript-only mode
    /// (PRD R3 — transcript-only is fully supported).
    SelectNotesModel {
        /// Registry id of the chosen notes model, or `None` to disable notes.
        model: Option<ModelId>,
    },

    /// Rename a speaker (click-to-edit), persisted; may trigger merge-by-rename
    /// (Appendix A row 14).
    RenameSpeaker {
        /// The speaker's current cluster id.
        speaker_id: u32,
        /// The new display name.
        name: String,
    },

    /// Cycle the timestamp display mode (elapsed / clock / ago — Appendix A
    /// row 24).
    CycleTimestampMode,
}

/// An event emitted by the core to the UI to render.
///
/// Carries engine activity ([`SessionEvent::Engine`]) plus session-level state
/// transitions the orchestrator owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    /// The recording session changed state (start/stop/resume/new-meeting).
    StateChanged {
        /// The new session state.
        state: SessionState,
    },

    /// An engine lifecycle event to render (transcript, progress, stats).
    /// Wraps the engine-level [`EngineEvent`] so the UI has one event stream.
    Engine(EngineEvent),

    /// A speaker label changed (rename / merge / recluster — Appendix A rows
    /// 13–15).
    SpeakerLabel {
        /// The speaker's cluster id.
        speaker_id: u32,
        /// The display name to show.
        name: String,
    },

    /// A non-fatal error to surface as a toast without aborting the session
    /// (for example a queued mid-recording engine switch). Fatal errors abort
    /// the call and are returned as `Result::Err`, not emitted here.
    Notice {
        /// Human-readable message.
        message: String,
    },

    /// A fatal error occurred; carries the shared [`AsrError`] so the UI shows a
    /// precise reason (PRD R4: a model-resolution error, not a broken UI).
    Error(AsrError),
}

/// The recording-session state machine's externally-visible state (Appendix A
/// rows 1–3). The full machine lives in `silent-core`'s orchestrator (Phase 4);
/// this enum is the UI-facing projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionState {
    /// No recording; ready to start.
    Idle,
    /// Loading the selected engine before capture begins.
    Loading,
    /// Actively recording and transcribing.
    Recording,
    /// Stopped; transcript and notes are final.
    Stopped,
}
