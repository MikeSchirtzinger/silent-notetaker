//! Meeting-history search — the last-50 list and the title/notes/transcript
//! search, as a pure, DOM-/IndexedDB-free policy module (Appendix A row 29;
//! PRD R2: "history/search ... are Rust").
//!
//! This is a byte-identical port of the JavaScript in `index.html`
//! (`openHistory` → `db.meetings.orderBy('startTime').reverse().limit(50)`, and
//! `_filterHistory`). The shipping code runs the same predicate against Dexie
//! queries; this module runs it against typed records the storage layer
//! (`silent-storage`'s IndexedDB code, Task H2) supplies, so the ranking is
//! deterministically testable without a browser.
//!
//! # What "fuzzy" means here
//!
//! Despite the Appendix A label "fuzzy search," the CURRENT behavior is a
//! case-insensitive SUBSTRING match (`toLowerCase().includes(q)`) across three
//! fields in order: meeting title, then any of the meeting's notes, then any of
//! its transcript chunks. The first field that matches admits the meeting; the
//! result preserves the candidate order (newest-first by `start_time`). This port
//! reproduces that behavior exactly — improvements (edit-distance ranking,
//! highlighting) are a separate, later change, never folded into the parity port
//! (per the session standard: capture current behavior first, prove equality).
//!
//! Validated against JS-generated goldens under `goldens/search/` (see
//! `tests/search_golden.rs`).

/// Stable meeting identifier (the Dexie `++id` auto-increment key).
pub type MeetingId = i64;

/// A meeting row as the search consumes it: id, title, and start time (epoch ms).
/// Mirrors the Dexie `meetings` store fields the search reads
/// (`{ id, title, startTime }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingRecord {
    /// The meeting's stable id.
    pub id: MeetingId,
    /// The meeting title (may be empty → "Untitled" in the UI).
    pub title: String,
    /// Recording start time, epoch milliseconds (the `startTime` sort key).
    pub start_time: i64,
}

/// A searchable text row scoped to a meeting — either a note or a transcript
/// chunk. The JS searches `db.notes` then `db.transcriptChunks`, both keyed by
/// `meetingId` with a `text` field; this one type carries both since the search
/// predicate is identical (case-insensitive substring on `text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRow {
    /// The meeting this row belongs to.
    pub meeting_id: MeetingId,
    /// The row's text (note text or transcript-chunk text).
    pub text: String,
}

/// The most-recent-meetings cap the history modal uses
/// (`...orderBy('startTime').reverse().limit(50)`).
pub const HISTORY_LIMIT: usize = 50;

/// Compute the candidate set: the most-recent `limit` meetings, newest first by
/// `start_time`, reproducing `db.meetings.orderBy('startTime').reverse().limit(n)`.
///
/// The sort is stable; ties in `start_time` keep input order then are reversed,
/// matching Dexie's behavior of reversing the ascending index scan.
#[must_use]
pub fn recent_meetings(meetings: &[MeetingRecord], limit: usize) -> Vec<&MeetingRecord> {
    let mut idx: Vec<usize> = (0..meetings.len()).collect();
    // Ascending by start_time, stable on ties (sort_by is stable).
    idx.sort_by(|&a, &b| meetings[a].start_time.cmp(&meetings[b].start_time));
    // `.reverse()` then `.limit(n)`: newest first, capped.
    idx.reverse();
    idx.into_iter().take(limit).map(|i| &meetings[i]).collect()
}

/// Search the meeting history, returning matched meeting ids in display order
/// (newest-first within the last-`limit` window).
///
/// Pipeline, byte-identical to the JS:
/// 1. candidate set = [`recent_meetings`]`(meetings, limit)`;
/// 2. `q = query.trim().to_lowercase()`; an empty `q` returns ALL candidate ids
///    in order (the unfiltered list);
/// 3. otherwise, for each candidate in order, admit it on the first hit among:
///    title contains `q`, else any of its `text_rows` (notes then chunks)
///    contains `q`. (Notes and chunks are both `TextRow`s; the JS checks notes
///    before chunks, but the result is membership, not which field hit, so the
///    order of the two collections is irrelevant to the output.)
///
/// `text_rows` should contain the meeting's notes and transcript chunks; rows for
/// meetings outside the candidate set are simply never consulted.
#[must_use]
pub fn search_history(
    meetings: &[MeetingRecord],
    text_rows: &[TextRow],
    query: &str,
    limit: usize,
) -> Vec<MeetingId> {
    let candidates = recent_meetings(meetings, limit);
    let q = query.trim().to_lowercase();

    if q.is_empty() {
        return candidates.iter().map(|m| m.id).collect();
    }

    let mut out: Vec<MeetingId> = Vec::new();
    for m in candidates {
        if m.title.to_lowercase().contains(&q) {
            out.push(m.id);
            continue;
        }
        let text_hit = text_rows
            .iter()
            .any(|r| r.meeting_id == m.id && r.text.to_lowercase().contains(&q));
        if text_hit {
            out.push(m.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: MeetingId, title: &str, start: i64) -> MeetingRecord {
        MeetingRecord {
            id,
            title: title.to_owned(),
            start_time: start,
        }
    }
    fn row(meeting_id: MeetingId, text: &str) -> TextRow {
        TextRow {
            meeting_id,
            text: text.to_owned(),
        }
    }

    #[test]
    fn recent_is_newest_first_and_capped() {
        let ms = [m(1, "a", 100), m(2, "b", 300), m(3, "c", 200)];
        let got: Vec<_> = recent_meetings(&ms, 2).iter().map(|x| x.id).collect();
        assert_eq!(got, vec![2, 3]); // newest two: start 300 then 200
    }

    #[test]
    fn empty_query_returns_all_in_order() {
        let ms = [m(1, "a", 100), m(2, "b", 300)];
        assert_eq!(search_history(&ms, &[], "", HISTORY_LIMIT), vec![2, 1]);
        // whitespace trims to empty
        assert_eq!(search_history(&ms, &[], "   ", HISTORY_LIMIT), vec![2, 1]);
    }

    #[test]
    fn title_then_text_match() {
        let ms = [m(1, "Kickoff", 100), m(2, "Design", 200)];
        let rows = [row(1, "we chose Rust")];
        // title hit
        assert_eq!(search_history(&ms, &rows, "design", HISTORY_LIMIT), vec![2]);
        // text hit (no title hit)
        assert_eq!(search_history(&ms, &rows, "rust", HISTORY_LIMIT), vec![1]);
        // no hit
        assert!(search_history(&ms, &rows, "zzz", HISTORY_LIMIT).is_empty());
    }

    #[test]
    fn outside_window_meetings_are_excluded() {
        // 3 meetings, limit 2 → oldest (id 1) is not a candidate even if its text
        // matches.
        let ms = [m(1, "old", 100), m(2, "mid", 200), m(3, "new", 300)];
        let rows = [row(1, "needle"), row(3, "needle")];
        assert_eq!(search_history(&ms, &rows, "needle", 2), vec![3]);
    }
}
