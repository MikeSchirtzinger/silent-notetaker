//! Smart-question + notes boundary contracts (PRD Phase 3, Appendix A rows
//! 19, 21, 22).
//!
//! The typed messages the UI exchanges with the Rust notes/questions core
//! (`silent-notes`). Scheduling, type rotation, dedup, recap timing, and the
//! Qwen chunk/parse/dedup pipeline are **Rust policy** (`silent-notes`); the
//! Qwen worker (`question-worker.js`) only executes the model. This module names
//! the wire contract so the worker and UI are wired against a CONTRACT, not
//! invented shapes.
//!
//! Same conventions as [`crate::commands`] / [`crate::diarization`]:
//! `#[non_exhaustive]` tagged enums (`{ "tag": "...", "payload": ... }`),
//! `snake_case`, ts-rs export under `#[cfg(test)]`. Additive variants do not
//! break the UI (it has a wildcard `switch` arm per the A3 escape hatch).
//!
//! # Privacy (PRD R5)
//!
//! These types carry transcript *text* and generated *question/note* text only.
//! Transcript text crosses this boundary because the question worker runs the
//! model **locally** (WASM/WebGPU in a Web Worker); it never leaves the device.

use serde::{Deserialize, Serialize};

/// Which smart-question type the policy is asking the worker to generate.
///
/// The five rotation types ported from `SmartQ`'s `TYPES` array (index.html
/// `~2201`). The rotation order and the per-type system prompts are Rust policy
/// (`silent-notes`); this enum is the type label that travels on the wire and
/// renders in the teleprompter chip (`smartqType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionType {
    /// A sharp clarifying question — the unstated detail that bites later.
    Clarify,
    /// A pointed question that challenges a decision or surfaces an unpriced
    /// risk.
    Risk,
    /// A question that converts discussion into a concrete next step.
    Followup,
    /// A question about something important the conversation has not touched.
    Coverage,
    /// A question that develops a briefly-mentioned idea into something concrete.
    Deepen,
}

/// A command the UI / orchestrator issues to the questions+notes core (UI →
/// core).
///
/// Mirrors the index.html interactions: live transcript accumulation feeding the
/// teleprompter, the manual reroll button, the minimize/expand toggle, and the
/// two stop-time passes (question recap + final notes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionCommand {
    /// Feed a finalized transcript fragment to the live scheduler (`SmartQ.accumulate`).
    /// The core decides whether the timing/char gates fire and, if so, emits a
    /// [`QuestionEvent::GenerateRequest`] for the worker to run.
    AccumulateTranscript {
        /// The transcript fragment (already finalized text, not a partial).
        text: String,
        /// Milliseconds from session start, the scheduler's clock (replaces
        /// `Date.now()` so command-log replay is deterministic — PRD "The UI
        /// boundary").
        now_ms: u64,
    },

    /// The user pressed the reroll ("Suggest another") button. Forces a fresh
    /// generation regardless of the timing/char gates and expands the bar if it
    /// was minimized (Appendix A row 21).
    Reroll {
        /// Scheduler clock in milliseconds from session start.
        now_ms: u64,
    },

    /// The user toggled the teleprompter minimize/expand state. Expanding clears
    /// the new-question badge.
    ToggleMinimize,

    /// Reset all scheduling state for a new meeting (`SmartQ.reset`).
    Reset,

    /// Run the stop-time question recap over the whole transcript (Appendix A
    /// row 21 "stop-time recap"). One generation per enabled type, deduped, top
    /// 3 each.
    RequestRecap,

    /// Run the stop-time final-notes pass (Appendix A row 19): chunk the
    /// transcript, request one Qwen generation per chunk, parse + dedup.
    RequestFinalNotes,

    /// The worker returned generated text for a prior [`QuestionEvent::GenerateRequest`].
    /// Carries the request id so the policy can match it and apply its
    /// uniqueness/dedup rules.
    WorkerResult {
        /// The id of the [`QuestionEvent::GenerateRequest`] this answers.
        request_id: u32,
        /// The model's raw output text (the policy strips/normalizes it).
        text: String,
    },
}

/// An event the questions+notes core emits to the UI / worker (core → UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuestionEvent {
    /// The policy decided to generate. The worker should run the model with this
    /// transcript window + question type and reply with a
    /// [`QuestionCommand::WorkerResult`] carrying the same `request_id`. Policy
    /// (which type, when, the window) is decided here; the worker only executes.
    GenerateRequest {
        /// Correlates the eventual [`QuestionCommand::WorkerResult`].
        request_id: u32,
        /// The rolling transcript window to condition on (already WINDOW_CHARS-capped).
        window: String,
        /// The question type to generate (drives the worker's system prompt).
        kind: QuestionType,
    },

    /// A live question is ready to display in the teleprompter (`SmartQ._render`).
    QuestionReady {
        /// The question text to show.
        text: String,
        /// The type chip label (clarify / risk / follow-up …).
        kind: QuestionType,
        /// `true` when the bar is minimized, so the UI raises the new-question
        /// badge dot (Appendix A row 21 "new-question badge").
        badge: bool,
    },

    /// The teleprompter minimize/expand state changed (after a toggle or a
    /// reroll-driven expand).
    MinimizeChanged {
        /// `true` if now minimized.
        minimized: bool,
    },

    /// The stop-time question recap finished. Groups are per-type (each capped at
    /// 3), plus the still-open questions tracked live, in the order the UI
    /// renders them (Appendix A row 21).
    RecapReady {
        /// The per-type recap groups.
        groups: Vec<RecapGroup>,
    },

    /// The stop-time final-notes pass finished (Appendix A row 19). Carries the
    /// parsed, deduped notes in chunk order.
    FinalNotesReady {
        /// The extracted notes (chunk order, deduped).
        notes: Vec<QwenNote>,
    },
}

/// One per-type group of recap questions (`generateQuestionRecap`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RecapGroup {
    /// The display label for this group (for example `Clarifying`,
    /// `Devil's Advocate`).
    pub label: String,
    /// Up to 3 deduped questions for this group.
    pub questions: Vec<String>,
}

/// One parsed Qwen note (`parseQwenNotes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct QwenNote {
    /// The note category as the UI list id: `decisions`, `actions`,
    /// `keypoints`, or `questions`.
    pub cat: String,
    /// The note text (length-capped at 160 chars with a trailing `…`).
    pub text: String,
    /// The outline topic this note sits under (`None` until a `TOPIC|` line set
    /// one in the same chunk).
    pub topic: Option<String>,
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
    fn question_command_round_trips() {
        let cmd = QuestionCommand::AccumulateTranscript {
            text: "we should ship friday".into(),
            now_ms: 5000,
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        assert!(
            json.contains("\"tag\":\"accumulate_transcript\""),
            "tagged: {json}"
        );
        let back: QuestionCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cmd, back);
    }

    #[test]
    fn question_event_generate_request_round_trips() {
        let ev = QuestionEvent::GenerateRequest {
            request_id: 7,
            window: "rolling window".into(),
            kind: QuestionType::Risk,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"generate_request\""));
        assert!(json.contains("\"kind\":\"risk\""), "type tagged: {json}");
        let back: QuestionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn final_notes_event_round_trips() {
        let ev = QuestionEvent::FinalNotesReady {
            notes: vec![QwenNote {
                cat: "decisions".into(),
                text: "ship v2 in Q3".into(),
                topic: Some("Roadmap".into()),
            }],
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"final_notes_ready\""));
        let back: QuestionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }
}
