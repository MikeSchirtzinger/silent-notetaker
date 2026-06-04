//! Word-corrections boundary contracts (PRD Phase 3, Appendix A row 25).
//!
//! The typed messages the UI exchanges with the Rust corrections core
//! (`silent-notes`): the add/remove/set commands the corrections panel issues,
//! and the `CorrectionsChanged` event the core emits so the UI re-renders the
//! correction tags. Defined here so the UI wiring binds against a CONTRACT, not
//! invented shapes — exactly as the [`crate::notes`] types do for the trigger
//! policy.
//!
//! These types describe the *boundary*; the correction-application POLICY (the
//! regex escape, the case-insensitive global replace, the insertion-ordered
//! map) lives in `silent-notes` and is byte-identical to the current
//! `index.html` `applyCorrections` / `applyCorrectionsToTranscript` behavior,
//! pinned by goldens.
//!
//! Same conventions as [`crate::commands`] / [`crate::notes`]:
//! `#[non_exhaustive]` tagged enums (`{ "tag": "...", "payload": ... }`),
//! `snake_case`, ts-rs export under `#[cfg(test)]`. Additive variants do not
//! break the UI (it has a wildcard `switch` arm per the A3 escape hatch).
//!
//! # Privacy (PRD R5/R7)
//!
//! These types carry only the user-typed correction pairs (a mis-heard word and
//! its fix). They stay inside the browser between the UI and the Rust core.

use serde::{Deserialize, Serialize};

/// One word-correction pair: replace every (case-insensitive) occurrence of
/// `wrong` with `right`. Mirrors the index.html `corrections` object entry
/// (`corrections[wrong] = right`). The order corrections are *added* is
/// preserved (the JS `Object.entries` insertion order), so later corrections
/// see earlier ones' output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Correction {
    /// The mis-heard word/phrase to replace (matched literally, case-insensitive).
    pub wrong: String,
    /// The replacement text.
    pub right: String,
}

/// A command the UI issues to the corrections core (UI → core). The corrections
/// panel (Appendix A row 25) drives these; the core applies them to incoming
/// transcript text and echoes the updated map back via
/// [`CorrectionEvent::CorrectionsChanged`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CorrectionCommand {
    /// Add (or overwrite) a correction (index.html `addCorrection`:
    /// `corrections[wrong] = right`). Overwriting an existing `wrong` keeps its
    /// original insertion position, matching the JS object semantics.
    AddCorrection {
        /// The mis-heard word/phrase.
        wrong: String,
        /// The replacement text.
        right: String,
    },

    /// Remove a correction by its `wrong` key (index.html `removeCorrection`:
    /// `delete corrections[wrong]`).
    RemoveCorrection {
        /// The `wrong` key to remove.
        wrong: String,
    },

    /// Replace the entire correction map (e.g. on restore). Order is the array
    /// order given.
    SetCorrections {
        /// The full ordered correction list.
        corrections: Vec<Correction>,
    },
}

/// An event the corrections core emits to the UI (core → UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CorrectionEvent {
    /// The correction map changed (after add/remove/set). The UI re-renders the
    /// correction tags (index.html `renderCorrectionTags`) and re-pushes the map
    /// to the transcription worker (`pushCorrectionsToWorker`). Carries the full
    /// ordered map so the UI never tracks correction state itself.
    CorrectionsChanged {
        /// The full ordered correction list, insertion order preserved.
        corrections: Vec<Correction>,
    },
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
    fn correction_command_round_trips() {
        let cmd = CorrectionCommand::AddCorrection {
            wrong: "kuber netes".into(),
            right: "Kubernetes".into(),
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        assert!(
            json.contains("\"tag\":\"add_correction\""),
            "tagged: {json}"
        );
        let back: CorrectionCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cmd, back);
    }

    #[test]
    fn correction_event_round_trips() {
        let ev = CorrectionEvent::CorrectionsChanged {
            corrections: vec![Correction {
                wrong: "teh".into(),
                right: "the".into(),
            }],
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"corrections_changed\""));
        let back: CorrectionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn correction_pair_is_wrong_right() {
        let c = Correction {
            wrong: "jira".into(),
            right: "Linear".into(),
        };
        let json = serde_json::to_string(&c).expect("serialize");
        assert_eq!(json, r#"{"wrong":"jira","right":"Linear"}"#);
    }
}
