//! Export and copy formatting — notes Markdown, timestamp-aware transcript text,
//! the meeting-summary executive line, and the history-replay export, as pure,
//! DOM-free formatting policy (PRD R2: "export formatting and timestamp modes";
//! Appendix A row 30).
//!
//! This is a byte-identical port of the JavaScript in `index.html`
//! (`notesToMarkdown`, `copyTranscript`, `generateSummary`'s executive line,
//! `openMeetingDetail`'s replay markdown, and `copySummaryMarkdown`'s AI-notes
//! append). The shipping code reads its inputs from the DOM and formats dates via
//! `Intl`; the core takes the SAME values as typed records and emits the SAME
//! strings. Date and per-line stamp strings are inputs, not computed here — the
//! orchestrator/`silent-web` supplies them (via [`crate::timestamp`] for the
//! stamps), keeping this module DOM- and `Intl`-free.
//!
//! Every formatter is validated against JS-generated goldens under
//! `goldens/export/` (see `tests/export_golden.rs`).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

// `NoteCategory` is the single canonical note category enum, defined in
// [`crate::notes`] (the notes domain type referenced by `NoteCommand` /
// `NoteEvent` / `NoteCounters`). G1 and H3 originally each declared an
// equivalent enum; they are reconciled to one definition here. The export-only
// helpers (`ORDER`, `markdown_header`) live alongside the type in `notes.rs`.
pub use crate::notes::NoteCategory;

/// A note as the exporter consumes it: a category, its text, and (optionally) the
/// already-formatted per-line timestamp string the UI shows (`.note-time`
/// textContent). The exporter never re-derives the stamp — it formats whatever
/// the active [`crate::timestamp`] mode produced, exactly as the DOM holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteRecord {
    /// Which section this note belongs to.
    pub category: NoteCategory,
    /// The note's text.
    pub text: String,
    /// The formatted timestamp string, or `None` if the note has no stamp. Only
    /// used by [`notes_to_markdown`] when timestamps are shown.
    #[serde(default)]
    pub time: Option<String>,
}

/// One transcript line for [`transcript_text`]: the formatted stamp and the line
/// text, as the DOM holds them (`.transcript-time` / `.transcript-text`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptLine {
    /// Formatted timestamp string (may be empty).
    #[serde(default)]
    pub time: String,
    /// The line text.
    pub text: String,
}

/// One AI-generated note group for the summary append: a label and its items.
/// Mirrors the `.ai-notes-group` blocks `copySummaryMarkdown` walks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiNoteGroup {
    /// The group label (e.g. `Discussion`, `Action Items`).
    pub label: String,
    /// The group's items.
    pub items: Vec<AiNoteItem>,
}

/// One AI note item: optional category chip plus text. A chip renders as
/// `- **chip** — text`; no chip renders as `- text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiNoteItem {
    /// The category chip label (`Decision`, `Action`, `Key point`, `Question`),
    /// or `None` for a bare item.
    #[serde(default)]
    pub chip: Option<String>,
    /// The item text.
    pub text: String,
}

/// The title fallback used when the meeting title is empty/whitespace, matching
/// the JS `... || 'Meeting Notes'`.
const TITLE_FALLBACK: &str = "Meeting Notes";

/// Resolve the title with the JS `(title.trim()) || 'Meeting Notes'` fallback.
fn resolve_title(title: &str) -> &str {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        TITLE_FALLBACK
    } else {
        trimmed
    }
}

/// `notesToMarkdown`: structured notes as Markdown.
///
/// ```text
/// # {title}
/// **Date:** {date}  **Duration:** {duration}
///
/// ## Decisions
/// - [ts] text          (when `with_time` and the note has a stamp)
/// - text               (otherwise)
/// ...
/// ```
///
/// Behavior pinned from the JS, including its quirks:
/// - sections render in [`NoteCategory::ORDER`]; a section with no surviving
///   items is omitted entirely;
/// - each item is `- [ts] text` when `with_time` and the note's `time` is
///   non-empty, else `- text`;
/// - an item whose text (after stripping the `- ` / `- [ts] ` prefix) is empty
///   is dropped — reproducing the JS
///   `.filter(line => line.replace(/^- (\[[^\]]*\] )?/, '').length > 0)`;
/// - the whole result is right/left-trimmed (`md.trim()`), so an all-empty notes
///   set yields just the `# title` / date-duration header block.
///
/// `title` is the raw meeting-title input; `date` and `duration` are
/// already-formatted strings the orchestrator supplies.
#[must_use]
pub fn notes_to_markdown(
    title: &str,
    date: &str,
    duration: &str,
    notes: &[NoteRecord],
    with_time: bool,
) -> String {
    let resolved = resolve_title(title);
    let mut md = format!("# {resolved}\n**Date:** {date}  **Duration:** {duration}\n\n");

    for cat in NoteCategory::ORDER {
        let mut lines: Vec<String> = Vec::new();
        for n in notes.iter().filter(|n| n.category == cat) {
            let text = n.text.trim();
            let ts = n.time.as_deref().unwrap_or("").trim();
            let line = if with_time && !ts.is_empty() {
                format!("- [{ts}] {text}")
            } else {
                format!("- {text}")
            };
            // JS filter: drop the line if the body (after `- ` or `- [ts] `) is
            // empty. The prefix is always present here, so the body == `text`.
            if !text.is_empty() {
                lines.push(line);
            }
        }
        if !lines.is_empty() {
            md.push_str(cat.markdown_header());
            md.push('\n');
            md.push_str(&lines.join("\n"));
            md.push_str("\n\n");
        }
    }

    md.trim().to_owned()
}

/// History-replay export (`openMeetingDetail`): like [`notes_to_markdown`] but
/// items are ALWAYS `- text` (no per-line stamps) and notes carry no timestamp.
/// Empty sections are skipped; the result is trimmed.
///
/// The JS builds this directly from stored note records (not the DOM) and does
/// NOT apply the empty-text filter — a stored note with empty text would render
/// as a bare `- ` line. This port preserves that (no filtering) to stay
/// byte-identical to the replay path.
#[must_use]
pub fn history_replay_markdown(
    title: &str,
    date: &str,
    duration: &str,
    notes: &[NoteRecord],
) -> String {
    let resolved = resolve_title(title);
    let mut md = format!("# {resolved}\n**Date:** {date}  **Duration:** {duration}\n\n");

    for cat in NoteCategory::ORDER {
        let items: Vec<String> = notes
            .iter()
            .filter(|n| n.category == cat)
            .map(|n| format!("- {}", n.text))
            .collect();
        if !items.is_empty() {
            md.push_str(cat.markdown_header());
            md.push('\n');
            md.push_str(&items.join("\n"));
            md.push_str("\n\n");
        }
    }

    md.trim().to_owned()
}

/// The meeting-summary executive line, byte-identical to `generateSummary`:
///
/// - counts decisions / actions / open questions;
/// - builds `"{n} decision[s] made"`, `"{n} action item[s] assigned"`,
///   `"{n} open question[s]"` for the non-zero categories (singular when `n == 1`);
/// - if any part exists: `"{duration} meeting with {parts joined by ', '}."`;
/// - otherwise: `"{duration} meeting recorded. {total_words} words transcribed."`.
#[must_use]
pub fn executive_line(duration: &str, notes: &[NoteRecord], total_words: u64) -> String {
    let count = |cat: NoteCategory| notes.iter().filter(|n| n.category == cat).count();
    let d = count(NoteCategory::Decisions);
    let a = count(NoteCategory::Actions);
    let q = count(NoteCategory::Questions);

    let mut parts: Vec<String> = Vec::new();
    if d > 0 {
        parts.push(format!("{d} decision{} made", plural(d)));
    }
    if a > 0 {
        parts.push(format!("{a} action item{} assigned", plural(a)));
    }
    if q > 0 {
        parts.push(format!("{q} open question{}", plural(q)));
    }

    if parts.is_empty() {
        format!("{duration} meeting recorded. {total_words} words transcribed.")
    } else {
        format!("{duration} meeting with {}.", parts.join(", "))
    }
}

/// `copyTranscript`: join transcript lines into plain text.
///
/// Each line is `[{time}] {text}` when `with_time` and the line's `time` is
/// non-empty, else `{text}`; both `time` and `text` are trimmed first. Lines that
/// are empty after formatting are dropped (`.filter(Boolean)`), reproducing the
/// JS exactly. Lines are joined with `\n`.
#[must_use]
pub fn transcript_text(lines: &[TranscriptLine], with_time: bool) -> String {
    lines
        .iter()
        .map(|l| {
            let t = l.time.trim();
            let txt = l.text.trim();
            if with_time && !t.is_empty() {
                format!("[{t}] {txt}")
            } else {
                txt.to_owned()
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `copySummaryMarkdown`: append the additive AI Meeting Notes groups to a base
/// notes-Markdown document.
///
/// If `ai_groups` is empty the base is returned unchanged. Otherwise the JS
/// appends `\n\n## AI Meeting Notes (on-device · Qwen)\n`, then per group with at
/// least one item: `\n### {label}\n{items joined by '\n'}\n`, where each item is
/// `- **{chip}** — {text}` (chip present) or `- {text}`; finally the whole thing
/// is trimmed.
#[must_use]
pub fn summary_markdown_with_ai(base_md: &str, ai_groups: &[AiNoteGroup]) -> String {
    if ai_groups.is_empty() {
        return base_md.to_owned();
    }
    // `write!` into the buffer (rather than `push_str(&format!(...))`) avoids the
    // extra intermediate allocation clippy::format_push_string flags. Writing to a
    // `String` is infallible, so the `Result` is intentionally ignored. `Write`
    // is imported at module scope to satisfy clippy::items_after_statements.
    let mut md = String::from(base_md);
    md.push_str("\n\n## AI Meeting Notes (on-device · Qwen)\n");
    for g in ai_groups {
        let items: Vec<String> = g
            .items
            .iter()
            .map(|i| match &i.chip {
                Some(chip) => format!("- **{chip}** — {}", i.text),
                None => format!("- {}", i.text),
            })
            .collect();
        if !items.is_empty() {
            let _ = write!(md, "\n### {}\n{}\n", g.label, items.join("\n"));
        }
    }
    md.trim().to_owned()
}

/// The JS `n > 1 ? 's' : ''` plural suffix.
fn plural(n: usize) -> &'static str {
    if n > 1 { "s" } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(cat: NoteCategory, text: &str, time: Option<&str>) -> NoteRecord {
        NoteRecord {
            category: cat,
            text: text.to_owned(),
            time: time.map(str::to_owned),
        }
    }

    #[test]
    fn empty_title_falls_back() {
        assert_eq!(resolve_title("   "), TITLE_FALLBACK);
        assert_eq!(resolve_title(""), TITLE_FALLBACK);
        assert_eq!(resolve_title("  Hi  "), "Hi");
    }

    #[test]
    fn empty_notes_yield_header_only() {
        let md = notes_to_markdown("T", "D", "00:00", &[], true);
        assert_eq!(md, "# T\n**Date:** D  **Duration:** 00:00");
    }

    #[test]
    fn whitespace_note_is_dropped() {
        let notes = [
            note(NoteCategory::Decisions, "real", Some("00:01")),
            note(NoteCategory::Decisions, "   ", Some("00:02")),
        ];
        let md = notes_to_markdown("T", "D", "00:10", &notes, true);
        assert!(md.contains("- [00:01] real"));
        assert!(!md.contains("00:02"));
    }

    #[test]
    fn executive_singular_vs_plural() {
        let one = [note(NoteCategory::Decisions, "x", None)];
        assert_eq!(
            executive_line("01:00", &one, 0),
            "01:00 meeting with 1 decision made."
        );
        let two = [
            note(NoteCategory::Decisions, "x", None),
            note(NoteCategory::Decisions, "y", None),
        ];
        assert_eq!(
            executive_line("01:00", &two, 0),
            "01:00 meeting with 2 decisions made."
        );
    }

    #[test]
    fn executive_words_fallback() {
        let kp = [note(NoteCategory::Keypoints, "k", None)];
        assert_eq!(
            executive_line("00:30", &kp, 9),
            "00:30 meeting recorded. 9 words transcribed."
        );
    }

    #[test]
    fn ai_append_noop_when_empty() {
        assert_eq!(summary_markdown_with_ai("base", &[]), "base");
    }
}
