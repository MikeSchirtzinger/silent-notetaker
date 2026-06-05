//! Wasm-bindgen export + history-duration formatting surface (PRD Phase 4,
//! Task h3/x3; Appendix A rows 24, 30).
//!
//! Exposes the pure `silent_core` export formatters and the timestamp duration
//! formatter to the UI as part of the single `silent_web.js` pkg — the same
//! "one wasm binary, many surfaces" strangler-fig pattern as [`crate::notes`]
//! exposes the Qwen free functions. The JS glue (`exports-engine.js`) loads the
//! shared wasm-pack output and calls these; `index.html`'s DOM-walking export
//! POLICY (`notesToMarkdown`, the `generateSummary` executive line,
//! `copyTranscript`, `copySummaryMarkdown`'s AI append, `openMeetingDetail`'s
//! replay markdown, and `formatDuration`) is removed when this is wired.
//!
//! # What moved off the DOM
//!
//! The DOM still SUPPLIES the inputs the formatters need (the visible note/
//! transcript text + the already-formatted per-line stamp strings the active
//! [`silent_core::timestamp`] mode produced, plus the locale `date`/`duration`
//! strings the browser computes via `Intl`). Only the FORMATTING — section
//! ordering, the empty-text filter, the `- [ts] text` shape, the executive
//! singular/plural line, the AI-notes append, the `Nm Ns` duration — moved into
//! Rust, where it is byte-identically golden-tested (`silent-core`'s
//! `tests/export_golden.rs` / `tests/timestamp_golden.rs`).
//!
//! # Wire format
//!
//! The DTO inputs (`NoteRecord` / `TranscriptLine` / `AiNoteGroup`) are passed
//! as JSON strings the glue `JSON.stringify`s; the formatters return plain
//! `String`s. This mirrors the `dedupeNotes` convention in [`crate::notes`]
//! (serde JSON in, value out) and avoids a `serde-wasm-bindgen` dep here.
//!
//! # wasm32-only
//!
//! Gated out of the native workspace build (see `lib.rs`); `cargo check
//! --workspace` stays browser-dep-free.

use wasm_bindgen::prelude::*;

use silent_core::export::{AiNoteGroup, NoteRecord, TranscriptLine};

/// Build a `JsError` from any `Display` error (a loud failure, never a silent
/// drop — an ill-formed input array rejects rather than exporting garbage).
fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// `notesToMarkdown`: structured notes as Markdown (Appendix A row 30).
///
/// `notes_json` is a JSON array of `{ category, text, time }` `NoteRecord`s the
/// glue assembles from the DOM (`category` ∈ the four `snake_case` keys; `time`
/// is the already-formatted `.note-time` stamp string or `null`). `date` and
/// `duration` are the locale strings the browser computed. `with_time` is the
/// `showTimestamps` toggle.
///
/// # Errors
///
/// Returns a `JsError` if `notes_json` is not a valid `NoteRecord` array.
#[wasm_bindgen(js_name = notesToMarkdown)]
pub fn notes_to_markdown(
    title: &str,
    date: &str,
    duration: &str,
    notes_json: &str,
    with_time: bool,
) -> Result<String, JsError> {
    let notes: Vec<NoteRecord> = serde_json::from_str(notes_json).map_err(to_js_err)?;
    Ok(silent_core::export::notes_to_markdown(
        title, date, duration, &notes, with_time,
    ))
}

/// History-replay export (`openMeetingDetail`): like [`notes_to_markdown`] but
/// items are ALWAYS `- text` (no per-line stamps), no empty-text filter.
///
/// `notes_json` is the same `NoteRecord` array shape (the stored note rows the
/// detail read returns; `time` is ignored on this path).
///
/// # Errors
///
/// Returns a `JsError` if `notes_json` is not a valid `NoteRecord` array.
#[wasm_bindgen(js_name = historyReplayMarkdown)]
pub fn history_replay_markdown(
    title: &str,
    date: &str,
    duration: &str,
    notes_json: &str,
) -> Result<String, JsError> {
    let notes: Vec<NoteRecord> = serde_json::from_str(notes_json).map_err(to_js_err)?;
    Ok(silent_core::export::history_replay_markdown(
        title, date, duration, &notes,
    ))
}

/// The meeting-summary executive line (`generateSummary`): the
/// `"{duration} meeting with {n decisions made, …}."` (or the words-transcribed
/// fallback) line, singular/plural-aware.
///
/// `notes_json` is the `NoteRecord` array (only `category` is consulted);
/// `total_words` is the running word count the fallback uses.
///
/// # Errors
///
/// Returns a `JsError` if `notes_json` is not a valid `NoteRecord` array.
#[wasm_bindgen(js_name = executiveLine)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "total_words is the DOM running word count: a finite, non-negative \
              integer far below u64::MAX; the guarded f64 → u64 cast is exact for \
              it. Non-finite / negative inputs clamp to 0"
)]
pub fn executive_line(
    duration: &str,
    notes_json: &str,
    total_words: f64,
) -> Result<String, JsError> {
    let notes: Vec<NoteRecord> = serde_json::from_str(notes_json).map_err(to_js_err)?;
    let words = if total_words.is_finite() && total_words >= 0.0 {
        total_words as u64
    } else {
        0
    };
    Ok(silent_core::export::executive_line(duration, &notes, words))
}

/// `copyTranscript`: join transcript lines into plain text, timestamp-aware
/// (Appendix A row 30).
///
/// `lines_json` is a JSON array of `{ time, text }` `TranscriptLine`s the glue
/// reads from the DOM (`time` is the already-formatted stamp). `with_time` is
/// the `showTimestamps` toggle.
///
/// # Errors
///
/// Returns a `JsError` if `lines_json` is not a valid `TranscriptLine` array.
#[wasm_bindgen(js_name = transcriptText)]
pub fn transcript_text(lines_json: &str, with_time: bool) -> Result<String, JsError> {
    let lines: Vec<TranscriptLine> = serde_json::from_str(lines_json).map_err(to_js_err)?;
    Ok(silent_core::export::transcript_text(&lines, with_time))
}

/// `copySummaryMarkdown`: append the additive AI Meeting Notes groups to a base
/// notes-Markdown document.
///
/// `groups_json` is a JSON array of `{ label, items:[{ chip, text }] }`
/// `AiNoteGroup`s walked from the `.ai-notes-group` DOM; an empty array returns
/// `base_md` unchanged.
///
/// # Errors
///
/// Returns a `JsError` if `groups_json` is not a valid `AiNoteGroup` array.
#[wasm_bindgen(js_name = summaryMarkdownWithAi)]
pub fn summary_markdown_with_ai(base_md: &str, groups_json: &str) -> Result<String, JsError> {
    let groups: Vec<AiNoteGroup> = serde_json::from_str(groups_json).map_err(to_js_err)?;
    Ok(silent_core::export::summary_markdown_with_ai(
        base_md, &groups,
    ))
}

/// The history-list duration string (`formatDuration`): `Nm Ns` from a
/// millisecond duration (Appendix A row 24). Not zero-padded, unlike the per-
/// line stamps.
///
/// `ms` is a JS number (the stored `meeting.duration`); fractional/NaN inputs
/// are floored toward zero defensively before the byte-identical port runs.
#[wasm_bindgen(js_name = formatDuration)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "ms is the stored meeting.duration (a finite, non-negative \
              millisecond integer far below i64::MAX); the guarded f64 → i64 cast \
              is exact for it. Non-finite inputs clamp to 0"
)]
#[must_use]
pub fn format_duration(ms: f64) -> String {
    let ms = if ms.is_finite() { ms as i64 } else { 0 };
    silent_core::timestamp::format_duration(ms)
}
