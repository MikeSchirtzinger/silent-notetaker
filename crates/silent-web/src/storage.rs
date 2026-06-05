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
/// # Errors
/// Rejects with a string on a read failure.
#[wasm_bindgen]
pub async fn recent_meetings() -> Result<JsValue, JsValue> {
    let db = silent_storage::writer::open_db().await.map_err(err)?;
    let mut meetings = silent_storage::reader::read_all(&db)
        .await
        .map_err(err)?
        .meetings;
    meetings.sort_by(|a, b| {
        b.start_time
            .partial_cmp(&a.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    meetings.truncate(silent_storage::search::HISTORY_LIMIT);
    silent_storage::summary::meetings_to_js(&meetings).map_err(|e| err(format!("{e:?}")))
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
