//! Notes boundary contracts (PRD Phase 3, Appendix A rows 16, 18).
//!
//! The typed messages the UI exchanges with the Rust notes core
//! (`silent-notes`): the live trigger-detection toggle, the categorized notes
//! the `NoteExtractor` emits, the live section counters, and the
//! open-question resolution events. Defined here so the UI wiring (a later
//! task) binds against a CONTRACT, not invented shapes — exactly as the
//! [`crate::diarization`] types do for Phase 2.
//!
//! These types describe the *boundary*; the trigger-extraction POLICY (the
//! regexes, the sentence buffer, the open-question keyword overlap) lives in
//! `silent-notes` and is byte-identical to the current `index.html` `NoteEngine`
//! / `OpenQs` behavior, pinned by goldens.
//!
//! Same conventions as [`crate::commands`] / [`crate::diarization`]:
//! `#[non_exhaustive]` tagged enums (`{ "tag": "...", "payload": ... }`),
//! `snake_case`, ts-rs export under `#[cfg(test)]`. Additive variants do not
//! break the UI (it has a wildcard `switch` arm per the A3 escape hatch).
//!
//! # Privacy (PRD R5/R7)
//!
//! These types carry transcript-derived *note text* only. Note text may leave
//! the browser solely through explicit user action (export, an approved
//! extension, or the local bridge); these boundary messages stay inside the
//! browser between the UI and the Rust core.

use serde::{Deserialize, Serialize};

/// The note category the trigger policy assigns. The four sections the UI
/// renders (Appendix A row 16): decisions, action items, key points, open
/// questions. Serialized `snake_case` to match the index.html category keys
/// (`decisions` / `actions` / `keypoints` / `questions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NoteCategory {
    /// A decision was reached ("we have decided to …").
    Decisions,
    /// An action item / task ("Alice will handle …").
    Actions,
    /// A substantive key point (metrics, results, "the problem is …").
    Keypoints,
    /// An open / unanswered question (the line is a question, "open question",
    /// "wondering if …"). Tracked for resolution (see [`NoteEvent::QuestionResolved`]).
    Questions,
}

/// A command the UI issues to the notes core (UI → core).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NoteCommand {
    /// Feed a final (accurate) transcript line to the trigger extractor. The
    /// core buffers, splits into sentences, categorizes, and resolves any prior
    /// open questions this line answers (index.html `addTranscript` →
    /// `noteEngine.analyze` + `OpenQs.consider`). Only sent when trigger
    /// detection is enabled.
    AnalyzeLine {
        /// The final transcript text for this line.
        text: String,
    },

    /// Flush the trailing sentence buffer at stop (index.html `stop()` →
    /// `noteEngine.flush`). Emits any trigger note still held in the buffer.
    Flush,

    /// Toggle live trigger detection (Appendix A row 18). When off, the UI
    /// stops sending [`NoteCommand::AnalyzeLine`]; the core keeps no extra
    /// state. Carried as a command so the setting is owned by Rust policy.
    SetTriggerDetection {
        /// `true` enables live trigger notes; `false` disables them.
        enabled: bool,
    },
}

/// An event the notes core emits to the UI (core → UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NoteEvent {
    /// A trigger note was created. The UI appends it to the matching section and
    /// bumps the live counter (Appendix A row 16). `id` is the stable note id the
    /// UI uses for edit/recategorize/delete (Appendix A row 17).
    NoteAdded {
        /// Stable note id (assigned by the core / storage).
        id: u64,
        /// The category section to render this note in.
        category: NoteCategory,
        /// The triggering sentence text.
        text: String,
        /// The regex that matched, as its source string — preserved
        /// byte-identically from the index.html `pattern.source` (kept for
        /// provenance / debugging; not rendered).
        trigger_phrase: String,
    },

    /// A previously-open question was answered by a later declarative line and
    /// should be struck through (index.html `OpenQs.consider` → `q-resolved`).
    /// The Open Questions counter shows only still-open questions.
    QuestionResolved {
        /// The note id of the question that is now resolved.
        id: u64,
    },

    /// The live section counters changed. `questions` is the count of *open*
    /// (unresolved) questions — matching the index.html behavior where
    /// `OpenQs._updateCount` overwrites the raw question-section count.
    CountersChanged {
        /// Current section counters.
        counters: NoteCounters,
    },
}

/// The four live note-section counters the UI renders (Appendix A row 16,
/// "+ live counters"). `decisions` / `actions` / `keypoints` are running note
/// counts in that section; `questions` is the count of *unresolved* open
/// questions (the index.html `OpenQs` semantics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct NoteCounters {
    /// Number of decision notes.
    pub decisions: u32,
    /// Number of action-item notes.
    pub actions: u32,
    /// Number of key-point notes.
    pub keypoints: u32,
    /// Number of *open* (unresolved) questions.
    pub questions: u32,
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism (PRD lint config)"
)]
mod tests {
    use super::*;

    #[test]
    fn note_category_serializes_to_index_html_keys() {
        // The four section keys must match index.html's `triggers` object keys
        // and `_noteSectionIds` map so the UI renders into the right section.
        for (cat, key) in [
            (NoteCategory::Decisions, "\"decisions\""),
            (NoteCategory::Actions, "\"actions\""),
            (NoteCategory::Keypoints, "\"keypoints\""),
            (NoteCategory::Questions, "\"questions\""),
        ] {
            assert_eq!(serde_json::to_string(&cat).expect("serialize"), key);
        }
    }

    #[test]
    fn note_command_round_trips() {
        let cmd = NoteCommand::AnalyzeLine {
            text: "we have decided to ship.".into(),
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        assert!(json.contains("\"tag\":\"analyze_line\""), "tagged: {json}");
        let back: NoteCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cmd, back);
    }

    #[test]
    fn note_event_round_trips() {
        let ev = NoteEvent::NoteAdded {
            id: 7,
            category: NoteCategory::Decisions,
            text: "we have decided to ship.".into(),
            trigger_phrase: "we('ve| have) decided to".into(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"note_added\""));
        let back: NoteEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn counters_default_is_all_zero() {
        let c = NoteCounters::default();
        assert_eq!(
            c,
            NoteCounters {
                decisions: 0,
                actions: 0,
                keypoints: 0,
                questions: 0,
            }
        );
    }
}
