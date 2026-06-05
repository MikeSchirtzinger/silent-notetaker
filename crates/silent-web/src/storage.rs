//! Wasm-bindgen browser-storage surface (PRD Phase 4, Task h2/x2; Appendix A
//! rows 1, 3, 16, 17, 19, 26, 27, 29, 33, plus the Phase-F durable-speaker-names
//! carry-forward).
//!
//! Exposes `silent-storage` to the UI as part of the single `silent_web.js` pkg —
//! the same strangler-fig pattern as [`crate::session`] wraps the session
//! machine. The JS glue (`storage-engine.js`) loads the shared wasm-pack output
//! and drives these functions; `index.html`'s direct Dexie calls (the Dexie
//! `<script>` itself) are removed when this is wired.
//!
//! # What moved off Dexie
//!
//! Every live read/write the recording app made through Dexie now routes here:
//!
//! - meetings: [`add_meeting`] (`App.start`), [`update_meeting_end`] (`App.stop`)
//! - transcriptChunks: [`add_transcript_chunk`] (`App.handleFinal`)
//! - notes: [`add_note`], [`update_note_text`], [`update_note_category`],
//!   [`delete_note`] (row 17 edit/recategorize/delete, now persisted)
//! - screenshots: [`add_screenshot`], [`mark_screenshot_analyzed`] (row 27 bridge
//!   analysis), [`count_screenshots`] (bridge summary)
//! - history: [`recent_meetings`], [`meeting_detail`]
//! - migration: [`migrate_database`], [`read_database_summary`]
//! - durable speaker names (Phase-F): [`save_speaker_names`], [`load_speaker_names`]
//!
//! The policy — the IndexedDB access, the schema ownership (v3 with the new
//! `speakerNames` store), the zero-loss migration, the export-backup-before-write
//! — all lives in `silent-storage`. This module is pure wasm-bindgen glue.
//!
//! # wasm32-only
//!
//! Gated out of the native workspace build (see `lib.rs`); `cargo check
//! --workspace` stays browser-dep-free.

use wasm_bindgen::prelude::*;

/// Stringify any error to a `JsValue` for a rejected `Promise`.
fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// `Meeting.start_time` (epoch-ms `f64`) → `i64` ms for the search policy's
/// integer `MeetingRecord::start_time`. The recording path always writes an
/// integer epoch-ms; a non-finite value is clamped to 0 rather than panicking
/// the read path.
#[allow(
    clippy::cast_possible_truncation,
    reason = "start_time is a Date.now() epoch-millisecond timestamp (finite, far \
              below i64::MAX); the guarded f64 → i64 cast is exact for it. \
              Non-finite inputs clamp to 0"
)]
fn start_ms(start_time: f64) -> i64 {
    if start_time.is_finite() {
        start_time as i64
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Migration + readback (productionized H2 — re-exposed through silent-web's pkg)
// ---------------------------------------------------------------------------

/// Run the Dexie v2 → Rust zero-loss migration. `on_event` is invoked with a
/// JSON string per [`silent_core::storage::StorageEvent`] — including the
/// `backup_ready` event the UI wires to an `<a download>` BEFORE any write.
/// Resolves to the after-migration counts; rejects with a string on failure.
///
/// # Errors
///
/// Rejects with a string if the migration fails at any step.
#[wasm_bindgen]
pub async fn migrate_database(on_event: js_sys::Function) -> Result<JsValue, JsValue> {
    console_error_panic_hook::set_once();

    let emit = move |ev: silent_core::storage::StorageEvent| match serde_json::to_string(&ev) {
        Ok(json) => {
            let _ = on_event.call1(&JsValue::NULL, &JsValue::from_str(&json));
        }
        Err(e) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "[silent-storage] event serialize failed: {e}"
            )));
        }
    };

    let counts = silent_storage::migrate::run_migration(&emit)
        .await
        .map_err(err)?;
    // Return the after-migration counts as a JSON string the glue `JSON.parse`s —
    // the same wire format as the rest of the silent-web boundary (avoids a
    // serde-wasm-bindgen dep here; the counts struct is small + plain).
    let json = serde_json::to_string(&counts).map_err(|e| err(format!("counts serialize: {e}")))?;
    Ok(JsValue::from_str(&json))
}

/// Read all four tables and return a JS summary object (counts + per-table data
/// + screenshot encodings) for the migration smoke check / before-after proof.
///
/// # Errors
///
/// Rejects with a string if the read fails.
#[wasm_bindgen]
pub async fn read_database_summary() -> Result<JsValue, JsValue> {
    console_error_panic_hook::set_once();
    let snapshot = silent_storage::reader::read_database().await.map_err(err)?;
    silent_storage::summary::snapshot_to_summary(&snapshot).map_err(|e| err(format!("{e:?}")))
}

// ---------------------------------------------------------------------------
// Live CRUD
// ---------------------------------------------------------------------------

/// `db.meetings.add(...)`. Returns the new meeting id.
///
/// # Errors
/// Rejects with a string on a write failure.
#[wasm_bindgen]
pub async fn add_meeting(title: String, start_time: f64) -> Result<JsValue, JsValue> {
    let id = silent_storage::writer::add_meeting(&title, start_time)
        .await
        .map_err(err)?;
    Ok(JsValue::from_f64(f64::from(id)))
}

/// `db.meetings.update(id, { endTime, duration })` (the Stop write).
///
/// # Errors
/// Rejects with a string if the meeting is missing or the write fails.
#[wasm_bindgen]
pub async fn update_meeting_end(
    meeting_id: u32,
    end_time: f64,
    duration: f64,
) -> Result<(), JsValue> {
    silent_storage::writer::update_meeting_end(meeting_id, end_time, duration)
        .await
        .map_err(err)
}

/// `db.transcriptChunks.add(...)`. Returns the new chunk id.
///
/// # Errors
/// Rejects with a string on a write failure.
#[wasm_bindgen]
pub async fn add_transcript_chunk(
    meeting_id: u32,
    timestamp: f64,
    text: String,
) -> Result<JsValue, JsValue> {
    let id = silent_storage::writer::add_transcript_chunk(meeting_id, timestamp, &text)
        .await
        .map_err(err)?;
    Ok(JsValue::from_f64(f64::from(id)))
}

/// `db.notes.add(...)`. Returns the new note id.
///
/// # Errors
/// Rejects with a string on a write failure.
#[wasm_bindgen]
pub async fn add_note(
    meeting_id: u32,
    category: String,
    text: String,
    timestamp: f64,
    trigger_phrase: String,
) -> Result<JsValue, JsValue> {
    let id =
        silent_storage::writer::add_note(meeting_id, &category, &text, timestamp, &trigger_phrase)
            .await
            .map_err(err)?;
    Ok(JsValue::from_f64(f64::from(id)))
}

/// `db.notes.update(id, { text })` (row 17 edit).
///
/// # Errors
/// Rejects with a string if the note is missing or the write fails.
#[wasm_bindgen]
pub async fn update_note_text(note_id: u32, text: String) -> Result<(), JsValue> {
    silent_storage::writer::update_note_field(note_id, "text", &text)
        .await
        .map_err(err)
}

/// `db.notes.update(id, { category })` (row 17 recategorize).
///
/// # Errors
/// Rejects with a string if the note is missing or the write fails.
#[wasm_bindgen]
pub async fn update_note_category(note_id: u32, category: String) -> Result<(), JsValue> {
    silent_storage::writer::update_note_field(note_id, "category", &category)
        .await
        .map_err(err)
}

/// `db.notes.delete(id)` (row 17 delete).
///
/// # Errors
/// Rejects with a string on a delete failure.
#[wasm_bindgen]
pub async fn delete_note(note_id: u32) -> Result<(), JsValue> {
    silent_storage::writer::delete_note(note_id)
        .await
        .map_err(err)
}

/// `db.screenshots.add(...)`. `image` is the raw bytes; `encoding` is the
/// `imageEncoding` marker (`"base64"` for the live data-URL-string capture path).
/// Returns the new screenshot id.
///
/// # Errors
/// Rejects with a string on a write failure.
#[wasm_bindgen]
pub async fn add_screenshot(
    meeting_id: u32,
    timestamp: f64,
    image: Vec<u8>,
    encoding: String,
    width: u32,
    height: u32,
) -> Result<JsValue, JsValue> {
    let id = silent_storage::writer::add_screenshot(
        meeting_id, timestamp, &image, &encoding, width, height,
    )
    .await
    .map_err(err)?;
    Ok(JsValue::from_f64(f64::from(id)))
}

/// `db.screenshots.where('timestamp').equals(ts).modify({ analyzed, analysis })`
/// (row 27 bridge analysis). Returns the count of rows updated.
///
/// # Errors
/// Rejects with a string on a write failure.
#[wasm_bindgen]
pub async fn mark_screenshot_analyzed(
    timestamp: f64,
    analysis: String,
) -> Result<JsValue, JsValue> {
    let n = silent_storage::writer::mark_screenshot_analyzed(timestamp, &analysis)
        .await
        .map_err(err)?;
    Ok(JsValue::from_f64(f64::from(n)))
}

/// `db.screenshots.where('meetingId').equals(id).count()` (bridge summary).
///
/// # Errors
/// Rejects with a string on a read failure.
#[wasm_bindgen]
pub async fn count_screenshots(meeting_id: u32) -> Result<JsValue, JsValue> {
    let n = silent_storage::writer::count_screenshots_for_meeting(meeting_id)
        .await
        .map_err(err)?;
    Ok(JsValue::from_f64(f64::from(n)))
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// Read all meetings newest-first, capped at the history limit (50). Returns a
/// JS array of `{ id, title, startTime, endTime, duration }`.
///
/// The newest-first/limit ranking is the [`silent_storage::search`] policy
/// (Appendix A row 29) — the SAME function the fuzzy search filters on — so the
/// initial list and the filtered list rank identically.
///
/// # Errors
/// Rejects with a string on a read failure.
#[wasm_bindgen]
pub async fn recent_meetings() -> Result<JsValue, JsValue> {
    let db = silent_storage::writer::open_db().await.map_err(err)?;
    let meetings = silent_storage::reader::read_all(&db)
        .await
        .map_err(err)?
        .meetings;
    let ranked = recent_meetings_ranked(&meetings);
    silent_storage::summary::meetings_to_js(&ranked).map_err(|e| err(format!("{e:?}")))
}

/// Search the meeting history (Appendix A row 29): case-insensitive substring
/// across title → notes → transcript chunks, within the last-50 newest-first
/// window. Returns a JS array of the matched meetings in display order — the
/// SAME `{ id, title, startTime, endTime, duration }` shape as
/// [`recent_meetings`], so the UI renders the filtered list with its existing
/// row renderer.
///
/// This runs the [`silent_storage::search::search_history`] policy in ONE DB
/// read (vs. the old JS N+1 per-meeting detail reads), with byte-identical
/// results: an empty/whitespace query returns the full recent list in order.
///
/// # Errors
/// Rejects with a string on a read failure.
#[wasm_bindgen]
pub async fn search_history(query: String) -> Result<JsValue, JsValue> {
    use silent_storage::search::{TextRow, search_history as run_search};

    let db = silent_storage::writer::open_db().await.map_err(err)?;
    let snap = silent_storage::reader::read_all(&db).await.map_err(err)?;

    // Project meetings onto the search policy's DTOs (ids coerce u32 → i64,
    // start_time f64 → i64 ms — the recording path always writes integer epoch-ms).
    let records = meeting_records(&snap.meetings);
    // Notes ++ transcript chunks are BOTH searched as `TextRow`s — the predicate
    // is identical (the JS checks notes then chunks; membership is the result).
    let text_rows: Vec<TextRow> = snap
        .notes
        .iter()
        .map(|n| TextRow {
            meeting_id: i64::from(n.meeting_id),
            text: n.text.clone(),
        })
        .chain(snap.transcript_chunks.iter().map(|c| TextRow {
            meeting_id: i64::from(c.meeting_id),
            text: c.text.clone(),
        }))
        .collect();

    let matched_ids = run_search(
        &records,
        &text_rows,
        &query,
        silent_storage::search::HISTORY_LIMIT,
    );

    // Re-materialize the matched meetings (full Meeting rows) in the policy's
    // returned order, so the UI's row renderer gets the same fields.
    let by_id: std::collections::HashMap<u32, &silent_storage::Meeting> =
        snap.meetings.iter().map(|m| (m.id, m)).collect();
    let out: Vec<silent_storage::Meeting> = matched_ids
        .iter()
        .filter_map(|id| {
            u32::try_from(*id)
                .ok()
                .and_then(|id| by_id.get(&id))
                .copied()
        })
        .cloned()
        .collect();
    silent_storage::summary::meetings_to_js(&out).map_err(|e| err(format!("{e:?}")))
}

/// Project storage `Meeting` rows onto the search policy's `MeetingRecord` DTOs
/// (id u32 → i64, `start_time` f64 → i64 ms).
fn meeting_records(
    meetings: &[silent_storage::Meeting],
) -> Vec<silent_storage::search::MeetingRecord> {
    meetings
        .iter()
        .map(|m| silent_storage::search::MeetingRecord {
            id: i64::from(m.id),
            title: m.title.clone(),
            start_time: start_ms(m.start_time),
        })
        .collect()
}

/// The newest-first, last-`HISTORY_LIMIT` candidate window, reusing the
/// [`silent_storage::search::recent_meetings`] policy (so the list ranking and
/// the search candidate set are the ONE function — they cannot drift). Returns
/// owned `Meeting` rows in display order for [`silent_storage::summary::meetings_to_js`].
fn recent_meetings_ranked(meetings: &[silent_storage::Meeting]) -> Vec<silent_storage::Meeting> {
    let records = meeting_records(meetings);
    let ranked =
        silent_storage::search::recent_meetings(&records, silent_storage::search::HISTORY_LIMIT);
    // Map the ranked records back to the full Meeting rows by id, preserving the
    // policy's order.
    let by_id: std::collections::HashMap<u32, &silent_storage::Meeting> =
        meetings.iter().map(|m| (m.id, m)).collect();
    ranked
        .iter()
        .filter_map(|r| {
            u32::try_from(r.id)
                .ok()
                .and_then(|id| by_id.get(&id))
                .copied()
        })
        .cloned()
        .collect()
}

/// Read one meeting's notes + transcript chunks for the history-detail replay
/// export. Returns `{ meeting, notes, chunks }`.
///
/// # Errors
/// Rejects with a string on a read failure.
#[wasm_bindgen]
pub async fn meeting_detail(meeting_id: u32) -> Result<JsValue, JsValue> {
    let db = silent_storage::writer::open_db().await.map_err(err)?;
    let snap = silent_storage::reader::read_all(&db).await.map_err(err)?;
    silent_storage::summary::meeting_detail_to_js(&snap, meeting_id)
        .map_err(|e| err(format!("{e:?}")))
}

// ---------------------------------------------------------------------------
// Durable speaker names (Phase-F carry-forward)
// ---------------------------------------------------------------------------

/// Persist a meeting's speaker rename map. `names_json` is a JSON object string
/// mapping raw speaker id (`"S1"`) → assigned name. An empty map clears the row.
///
/// # Errors
/// Rejects with a string on a JSON parse error or write failure.
#[wasm_bindgen]
pub async fn save_speaker_names(meeting_id: u32, names_json: String) -> Result<(), JsValue> {
    let names: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&names_json).map_err(|e| err(format!("names JSON: {e}")))?;
    silent_storage::writer::save_speaker_names(meeting_id, names)
        .await
        .map_err(err)
}

/// Load a meeting's speaker rename map as a JSON object string (`{}` if none).
///
/// # Errors
/// Rejects with a string on a read failure.
#[wasm_bindgen]
pub async fn load_speaker_names(meeting_id: u32) -> Result<JsValue, JsValue> {
    let names = silent_storage::writer::load_speaker_names(meeting_id)
        .await
        .map_err(err)?;
    let json = serde_json::to_string(&names).map_err(|e| err(format!("names ser: {e}")))?;
    Ok(JsValue::from_str(&json))
}
