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
    ///
    /// The title is clamped to 120 characters by the orchestrator (Appendix A
    /// row 3, matching the `maxlength="120"` HTML input); an empty/whitespace
    /// title falls back to the default. This is the *first* start of a meeting;
    /// the resume/continue path is [`UiCommand::ResumeRecording`].
    StartRecording {
        /// Meeting title (auto date/time default, up to 120 chars per
        /// Appendix A row 3).
        title: String,
    },

    /// Stop the active recording (triggers finalize + stop-time notes/recap).
    StopRecording,

    /// Resume/continue the previous recording without reloading the model
    /// (Appendix A row 2). Valid only from [`SessionState::Stopped`]; the
    /// orchestrator keeps the engine loaded so this is a warm restart.
    ResumeRecording,

    /// Reset to a fresh meeting (Appendix A row 3): clears session state,
    /// re-arms the auto date/time title, and returns to [`SessionState::Idle`].
    NewMeeting,

    /// Set/replace the meeting title before (or between) recordings. The
    /// orchestrator clamps it to 120 characters (Appendix A row 3) and keeps it
    /// as the pending title for the next [`UiCommand::StartRecording`].
    SetTitle {
        /// The proposed title; clamped to 120 chars, empty falls back to the
        /// default at start.
        title: String,
    },

    /// Add tab/system audio to the active recording (Appendix A rows 5–6). Only
    /// meaningful while [`SessionState::Recording`]; turns on the Tab Audio
    /// badge via a [`SessionEvent::SourcesChanged`].
    AddTabAudio,

    /// Remove tab/system audio from the active recording (the user toggled it
    /// off, or the shared stream ended — Appendix A rows 5–6).
    RemoveTabAudio,

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

    /// The active title changed (a [`UiCommand::SetTitle`] / [`UiCommand::NewMeeting`]
    /// auto-title was applied). The UI writes this into the `#meetingTitle`
    /// input. Always already clamped to 120 chars (Appendix A row 3).
    TitleChanged {
        /// The clamped, render-ready title.
        title: String,
    },

    /// The set of active capture sources changed (Appendix A row 6: the Mic /
    /// Tab Audio badges). The UI shows/hides each badge from these flags.
    SourcesChanged {
        /// Mic capture is active.
        mic: bool,
        /// Tab/system audio is mixed in.
        tab: bool,
    },

    /// The timestamp display mode changed (Appendix A row 24, the cycle button).
    TimestampModeChanged {
        /// The newly-active mode.
        mode: TimestampMode,
    },

    /// The recording stopped and the orchestrator computed which stop-time
    /// passes to run (Appendix A rows 15, 19, 21, 31). Each flag is a *typed
    /// trigger point*, not a result: the UI / host layer runs the corresponding
    /// pass (global recluster, AI final notes, smart-question recap, auto
    /// summary) when its flag is set. Decoupling the decision (Rust policy) from
    /// the execution (host) is the R2 "law vs hands" split.
    StopHooks(StopHooks),

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

/// The timestamp display mode for the header timer and per-line stamps
/// (Appendix A row 24). The cycle button rotates `elapsed → clock → ago →
/// elapsed`, matching the `timeFormat` setting in index.html.
///
/// The variant order is the cycle order: [`TimestampMode::next`] advances one
/// step. Serialized `snake_case` (`"elapsed"`, `"clock"`, `"ago"`) so it round-
/// trips with the persisted `timeFormat` localStorage value unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TimestampMode {
    /// `mm:ss` since recording start (the default; matches `'elapsed'`).
    #[default]
    Elapsed,
    /// Wall-clock `h:mm AM/PM` (matches `'clock'`).
    Clock,
    /// Relative `"30s ago"` / `"5m ago"` / `"2h ago"` (matches `'ago'`).
    Ago,
}

impl TimestampMode {
    /// Advance to the next mode in the cycle (`elapsed → clock → ago → elapsed`),
    /// matching index.html's cycle button (Appendix A row 24).
    ///
    /// Exhaustive on purpose: a future timestamp mode must be placed explicitly
    /// in the cycle order, so adding a `#[non_exhaustive]` variant makes this
    /// fail to compile rather than silently dropping it out of the rotation.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Elapsed => Self::Clock,
            Self::Clock => Self::Ago,
            Self::Ago => Self::Elapsed,
        }
    }

    /// The persisted `timeFormat` string this mode round-trips with
    /// (`"elapsed"` / `"clock"` / `"ago"`).
    ///
    /// Exhaustive (no wildcard): although the enum is `#[non_exhaustive]` for
    /// downstream crates, in-crate the compiler sees every variant, so adding
    /// one later makes this match fail to compile — forcing the new variant to
    /// get a label rather than silently defaulting.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Elapsed => "elapsed",
            Self::Clock => "clock",
            Self::Ago => "ago",
        }
    }
}

/// Which stop-time passes the orchestrator decided to trigger (Appendix A rows
/// 15, 19, 21, 31), emitted inside [`SessionEvent::StopHooks`].
///
/// These are *decisions*, computed from the session's [`SessionConfig`](crate::session::SessionConfig)
/// at Stop, exactly mirroring the guards in index.html's `stop()`:
///
/// - `recluster` ← a tracker is loaded with `> 1` speaker (DIARIZATION.md §2).
/// - `final_notes` ← `aiFinalNotes !== false`.
/// - `question_recap` ← `smartQuestions !== false && smartqRecap !== false`.
/// - `auto_summary` ← the summary modal always opens at Stop; `autoSummary`
///   additionally requests the on-device/bridge summary pass.
///
/// The host layer runs each enabled pass; Rust owns *whether* to (R2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[non_exhaustive]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent stop-time trigger point (recluster / \
              final notes / question recap / auto summary); a bitflags type would \
              obscure the 1:1 map to index.html's stop() guards and the UI's \
              per-pass rendering"
)]
pub struct StopHooks {
    /// Run the stop-time global speaker recluster (Appendix A row 15).
    pub recluster: bool,
    /// Run the Qwen AI final-notes pass (Appendix A row 19).
    pub final_notes: bool,
    /// Run the smart-question stop-time recap (Appendix A row 21).
    pub question_recap: bool,
    /// Run the auto-summary pass (Appendix A row 31).
    pub auto_summary: bool,
}
