//! The live CRUD writer (wasm32 only) — the Rust-owned replacement for the
//! app's direct Dexie calls (PRD Phase 4; Appendix A rows 1, 3, 17, 26, 29, 33,
//! plus the Phase-F carry-forward: durable speaker names).
//!
//! Dexie used to own every live read/write the recording app made
//! (`db.meetings.add`, `db.transcriptChunks.add`, `db.notes.add/update/delete`,
//! `db.screenshots.add/modify/count`, plus the history queries). This module is
//! the strangler-fig replacement: the same IndexedDB `SilentNotetaker` database,
//! driven from Rust via `indexed_db_futures`, behind a typed surface `silent-web`'s
//! `storage` module exposes to the glue (`storage-engine.js`). When this is
//! wired, the Dexie `<script>` is removed from `index.html` entirely.
//!
//! # Schema ownership (v3)
//!
//! Dexie created the DB at IDB version 20 (`db.version(2) × 10`). Now that Rust
//! owns the DB it opens at version 30 ([`silent_core::storage::RUST_SCHEMA_VERSION`]
//! `× 10`) and, in the upgrade callback, ensures every store exists:
//!
//! - the four Dexie v2 stores (`meetings`, `transcriptChunks`, `notes`,
//!   `screenshots`) are created with `autoIncrement` `++id` ONLY if absent (a
//!   fresh install with no prior Dexie DB), and left untouched on an existing DB
//!   — so opening a v2 database at v3 is non-destructive (the migration relies on
//!   this).
//! - the new `speakerNames` store ([`silent_core::storage::SPEAKER_NAMES_STORE`])
//!   is created keyed on `meetingId` (NOT auto-increment) for the durable
//!   per-meeting rename map.
//!
//! The Dexie ×10 rule still holds for the migration pre-flight: a never-migrated
//! v2 DB is at 20; this open bumps it to 30 the first time it is touched. The
//! migration (`crate::migrate`) opens version-less and asserts 20 BEFORE this
//! writer ever bumps it, so the ordering (migrate-then-write) is preserved by the
//! glue (it runs `migrate_database()` before any live write on first load).

use std::collections::BTreeMap;

use indexed_db_futures::database::Database;
use indexed_db_futures::prelude::*;
use indexed_db_futures::transaction::TransactionMode;
use js_sys::{Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use silent_core::storage::{
    EXTENSION_GRANTS_STORE, Note, RUST_SCHEMA_VERSION, SPEAKER_NAMES_STORE, SpeakerName,
    expected_idb_version,
};

use crate::error::{Result, StorageError};
use crate::reader::DB_NAME;

/// The four Dexie v2 stores, each with an auto-incrementing `id` key path.
const AUTOINC_STORES: [&str; 4] = ["meetings", "transcriptChunks", "notes", "screenshots"];

/// Open the `SilentNotetaker` database at the Rust-owned schema version (40),
/// creating any missing store in the upgrade callback.
///
/// Opening at a higher version than the DB's current one triggers a single
/// `upgradeneeded` where the stores are ensured. On a DB already at v4 this is a
/// plain open (no upgrade). On a fresh browser (no DB) it creates all six stores.
/// On a Dexie v2 DB (version 20) it bumps to 40 and adds only the missing
/// `speakerNames` + `extensionGrants` stores; on a v3 DB (version 30) it adds
/// only the missing `extensionGrants` store. The four data stores always exist
/// and are untouched.
///
/// # Errors
///
/// Returns [`StorageError::Open`] if the open or upgrade fails.
pub async fn open_db() -> Result<Database> {
    let version: u32 = expected_idb_version(RUST_SCHEMA_VERSION); // 30

    Database::open(DB_NAME)
        .with_version(version)
        .with_on_upgrade_needed(|_event, db| {
            // The upgrade callback wants an `indexed_db_futures::error::Error`;
            // wrap any creation failure's message in a `js_sys::Error` (which
            // converts in via `From<js_sys::Error>`) so it surfaces loudly rather
            // than panicking the upgrade.
            ensure_stores(&db).map_err(|e| js_sys::Error::new(&e.to_string()).into())
        })
        .build()
        .map_err(|e| StorageError::Open(format!("{e:?}")))?
        .await
        .map_err(|e| StorageError::Open(format!("{e:?}")))
}

/// Create every store that does not yet exist (runs inside `upgradeneeded`).
fn ensure_stores(db: &Database) -> Result<()> {
    let existing: Vec<String> = db.object_store_names().collect();
    let has = |name: &str| existing.iter().any(|n| n == name);

    for store in AUTOINC_STORES {
        if !has(store) {
            // Match Dexie's `'++id'` schema EXACTLY: inline auto-increment on the
            // `id` key path (the key is stored at `record.id`). A keyless
            // auto-increment store would diverge from what Dexie created and break
            // the migration's `put`-with-inline-id round-trip.
            db.create_object_store(store)
                .with_auto_increment(true)
                .with_key_path(indexed_db_futures::KeyPath::One("id"))
                .build()
                .map_err(|e| StorageError::Operation(format!("create {store}: {e:?}")))?;
        }
    }

    if !has(SPEAKER_NAMES_STORE) {
        // Keyed on `meetingId` (one row per meeting); NOT auto-increment — the
        // app supplies the key so a re-save `put`s over the same meeting's map.
        db.create_object_store(SPEAKER_NAMES_STORE)
            .with_key_path(indexed_db_futures::KeyPath::One("meetingId"))
            .build()
            .map_err(|e| StorageError::Operation(format!("create {SPEAKER_NAMES_STORE}: {e:?}")))?;
    }

    if !has(EXTENSION_GRANTS_STORE) {
        // Schema v4 (PRD Phase 6 / R7): one row per installed extension, keyed on
        // the extension `name` (a validated `[a-z0-9._-]` string id); NOT
        // auto-increment — the host supplies the key so re-installing/re-granting
        // `put`s over the same extension's row. The stored value is the
        // serde-serialized `GrantSet` as an opaque JSON string (storage never
        // depends on the extension SDK).
        db.create_object_store(EXTENSION_GRANTS_STORE)
            .with_key_path(indexed_db_futures::KeyPath::One("extension"))
            .build()
            .map_err(|e| {
                StorageError::Operation(format!("create {EXTENSION_GRANTS_STORE}: {e:?}"))
            })?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers for building IDB record objects field-by-field (the auto-increment
// stores must NOT carry an `id` on insert — IDB assigns it).
// ---------------------------------------------------------------------------

fn set(obj: &Object, key: &str, val: &JsValue) -> Result<()> {
    Reflect::set(obj, &JsValue::from_str(key), val)
        .map(|_| ())
        .map_err(|e| StorageError::Js(format!("Reflect::set({key}): {e:?}")))
}

/// `add` a value into an auto-increment store and return the generated `u32` id.
async fn add_autoinc(db: &Database, store_name: &str, record: &JsValue) -> Result<u32> {
    let tx = db
        .transaction([store_name])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store(store_name)?;
    let key: u32 = store
        .add(record)
        .with_key_type::<u32>()
        .primitive()
        .map_err(|e| StorageError::Operation(format!("add {store_name}: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("add await {store_name}: {e:?}")))?;
    tx.commit().await?;
    Ok(key)
}

// ---------------------------------------------------------------------------
// meetings (Appendix A rows 1, 3, 33)
// ---------------------------------------------------------------------------

/// `db.meetings.add({ title, startTime, endTime: null, duration: 0 })`.
/// Returns the new meeting id.
///
/// # Errors
///
/// Returns [`StorageError`] if the write fails.
pub async fn add_meeting(title: &str, start_time: f64) -> Result<u32> {
    let db = open_db().await?;
    let obj = Object::new();
    set(&obj, "title", &JsValue::from_str(title))?;
    set(&obj, "startTime", &JsValue::from_f64(start_time))?;
    set(&obj, "endTime", &JsValue::NULL)?;
    set(&obj, "duration", &JsValue::from_f64(0.0))?;
    add_autoinc(&db, "meetings", &obj.into()).await
}

/// `db.meetings.update(id, { endTime, duration })` — the Stop-time write.
///
/// Reads the existing row, patches `endTime`/`duration`, and `put`s it back so
/// the title and startTime are preserved.
///
/// # Errors
///
/// Returns [`StorageError`] if the meeting is missing or the write fails.
pub async fn update_meeting_end(meeting_id: u32, end_time: f64, duration: f64) -> Result<()> {
    let db = open_db().await?;
    let tx = db
        .transaction(["meetings"])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store("meetings")?;
    let existing: Option<JsValue> = store
        .get::<JsValue, u32, _>(meeting_id)
        .primitive()
        .map_err(|e| StorageError::Operation(format!("get meeting: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("get meeting await: {e:?}")))?;
    let row = existing.ok_or_else(|| {
        StorageError::Operation(format!("meeting {meeting_id} not found for update"))
    })?;
    let obj: Object = row.into();
    set(&obj, "endTime", &JsValue::from_f64(end_time))?;
    set(&obj, "duration", &JsValue::from_f64(duration))?;
    store
        .put(&JsValue::from(obj))
        .build()
        .map_err(|e| StorageError::Operation(format!("put meeting: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("put meeting await: {e:?}")))?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// transcriptChunks (Appendix A rows 29, 33)
// ---------------------------------------------------------------------------

/// `db.transcriptChunks.add({ meetingId, timestamp, text, isFinal: true })`.
///
/// # Errors
///
/// Returns [`StorageError`] if the write fails.
pub async fn add_transcript_chunk(meeting_id: u32, timestamp: f64, text: &str) -> Result<u32> {
    let db = open_db().await?;
    let obj = Object::new();
    set(&obj, "meetingId", &JsValue::from_f64(f64::from(meeting_id)))?;
    set(&obj, "timestamp", &JsValue::from_f64(timestamp))?;
    set(&obj, "text", &JsValue::from_str(text))?;
    set(&obj, "isFinal", &JsValue::TRUE)?;
    add_autoinc(&db, "transcriptChunks", &obj.into()).await
}

// ---------------------------------------------------------------------------
// notes (Appendix A rows 16, 17, 19, 33)
// ---------------------------------------------------------------------------

/// `db.notes.add({ meetingId, category, text, timestamp, triggerPhrase })`.
///
/// # Errors
///
/// Returns [`StorageError`] if the write fails.
pub async fn add_note(
    meeting_id: u32,
    category: &str,
    text: &str,
    timestamp: f64,
    trigger_phrase: &str,
) -> Result<u32> {
    let db = open_db().await?;
    let obj = Object::new();
    set(&obj, "meetingId", &JsValue::from_f64(f64::from(meeting_id)))?;
    set(&obj, "category", &JsValue::from_str(category))?;
    set(&obj, "text", &JsValue::from_str(text))?;
    set(&obj, "timestamp", &JsValue::from_f64(timestamp))?;
    set(&obj, "triggerPhrase", &JsValue::from_str(trigger_phrase))?;
    add_autoinc(&db, "notes", &obj.into()).await
}

/// Patch a single field on a note (`db.notes.update(id, { <field>: <value> })`)
/// — the row-17 edit (`text`) and recategorize (`category`) paths.
///
/// Reads, patches one string field, and `put`s back so the other fields survive.
///
/// # Errors
///
/// Returns [`StorageError`] if the note is missing or the write fails.
pub async fn update_note_field(note_id: u32, field: &str, value: &str) -> Result<()> {
    let db = open_db().await?;
    let tx = db
        .transaction(["notes"])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store("notes")?;
    let existing: Option<JsValue> = store
        .get::<JsValue, u32, _>(note_id)
        .primitive()
        .map_err(|e| StorageError::Operation(format!("get note: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("get note await: {e:?}")))?;
    let row =
        existing.ok_or_else(|| StorageError::Operation(format!("note {note_id} not found")))?;
    let obj: Object = row.into();
    set(&obj, field, &JsValue::from_str(value))?;
    store
        .put(&JsValue::from(obj))
        .build()
        .map_err(|e| StorageError::Operation(format!("put note: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("put note await: {e:?}")))?;
    tx.commit().await?;
    Ok(())
}

/// `db.notes.delete(id)` — the row-17 delete path.
///
/// # Errors
///
/// Returns [`StorageError`] if the delete fails.
pub async fn delete_note(note_id: u32) -> Result<()> {
    let db = open_db().await?;
    let tx = db
        .transaction(["notes"])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store("notes")?;
    store
        .delete::<u32, _>(note_id)
        .build()
        .map_err(|e| StorageError::Operation(format!("delete note: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("delete note await: {e:?}")))?;
    tx.commit().await?;
    Ok(())
}

/// Read all notes for a meeting (history detail replay).
///
/// # Errors
///
/// Returns [`StorageError`] if the read fails.
pub async fn notes_for_meeting(meeting_id: u32) -> Result<Vec<Note>> {
    let db = open_db().await?;
    let all = crate::reader::read_all(&db).await?.notes;
    Ok(all
        .into_iter()
        .filter(|n| n.meeting_id == meeting_id)
        .collect())
}

// ---------------------------------------------------------------------------
// screenshots (Appendix A rows 26, 27, 33)
// ---------------------------------------------------------------------------

/// `db.screenshots.add({ meetingId, timestamp, image: <bytes>, width, height,
/// analyzed: false, imageEncoding })`.
///
/// `image` is stored as a `Uint8Array` (the Rust-owned normalized layout), with
/// an `imageEncoding` marker recording whether the bytes are a base64 data-URL
/// STRING (the live-capture path: `encoding = "base64"`) or raw binary. This is
/// the SAME normalized representation the migration writes, so live captures and
/// migrated rows are indistinguishable to the render path.
///
/// # Errors
///
/// Returns [`StorageError`] if the write fails.
pub async fn add_screenshot(
    meeting_id: u32,
    timestamp: f64,
    image: &[u8],
    encoding: &str,
    width: u32,
    height: u32,
) -> Result<u32> {
    let db = open_db().await?;
    let obj = Object::new();
    set(&obj, "meetingId", &JsValue::from_f64(f64::from(meeting_id)))?;
    set(&obj, "timestamp", &JsValue::from_f64(timestamp))?;
    set(&obj, "image", &Uint8Array::from(image).into())?;
    set(&obj, "width", &JsValue::from_f64(f64::from(width)))?;
    set(&obj, "height", &JsValue::from_f64(f64::from(height)))?;
    set(&obj, "analyzed", &JsValue::FALSE)?;
    set(&obj, "imageEncoding", &JsValue::from_str(encoding))?;
    add_autoinc(&db, "screenshots", &obj.into()).await
}

/// Mark every screenshot at `timestamp` as analyzed with the bridge analysis
/// text (`db.screenshots.where('timestamp').equals(ts).modify({ analyzed: true,
/// analysis })`). Returns the number of rows updated.
///
/// # Errors
///
/// Returns [`StorageError`] if the read/write fails.
pub async fn mark_screenshot_analyzed(timestamp: f64, analysis: &str) -> Result<u32> {
    use futures::TryStreamExt as _;

    let db = open_db().await?;

    // ── Phase 1: read raw matching rows (read tx; the `image` JsValue — a
    // Uint8Array, a base64 string, or a Blob — is carried unchanged so the
    // re-`put` never rewrites the image bytes). The screenshots store is inline-
    // keyed on `id`, so each row already carries its key; `put` restores it.
    let mut matched: Vec<JsValue> = Vec::new();
    {
        let tx = db
            .transaction(["screenshots"])
            .with_mode(TransactionMode::Readonly)
            .build()?;
        let store = tx.object_store("screenshots")?;
        if let Some(cursor) = store.open_cursor().build()?.await? {
            let mut stream = cursor.stream::<JsValue>();
            while let Some(raw) = stream.try_next().await? {
                let ts = Reflect::get(&raw, &JsValue::from_str("timestamp"))
                    .ok()
                    .and_then(|v| v.as_f64());
                if ts == Some(timestamp) {
                    matched.push(raw);
                }
            }
        }
        tx.commit().await?;
    }

    if matched.is_empty() {
        return Ok(0);
    }

    // ── Phase 2: patch `analyzed`/`analysis` and `put` each matched row back.
    let tx = db
        .transaction(["screenshots"])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store("screenshots")?;
    let mut updated = 0u32;
    for raw in matched {
        let obj: Object = raw.into();
        set(&obj, "analyzed", &JsValue::TRUE)?;
        set(&obj, "analysis", &JsValue::from_str(analysis))?;
        store
            .put(&JsValue::from(obj))
            .build()
            .map_err(|e| StorageError::Operation(format!("put screenshot: {e:?}")))?
            .await
            .map_err(|e| StorageError::Operation(format!("put ss await: {e:?}")))?;
        updated += 1;
    }
    tx.commit().await?;
    Ok(updated)
}

/// Count screenshots for a meeting (the bridge summary's
/// `db.screenshots.where('meetingId').equals(id).count()`).
///
/// # Errors
///
/// Returns [`StorageError`] if the read fails.
pub async fn count_screenshots_for_meeting(meeting_id: u32) -> Result<u32> {
    let db = open_db().await?;
    let snapshot = crate::reader::read_all(&db).await?;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "screenshot count for one meeting fits u32"
    )]
    let n = snapshot
        .screenshots
        .iter()
        .filter(|s| s.meeting_id == meeting_id)
        .count() as u32;
    Ok(n)
}

// ---------------------------------------------------------------------------
// speakerNames — the Phase-F carry-forward (durable speaker rename map)
// ---------------------------------------------------------------------------

/// Persist the per-meeting speaker rename map
/// (`speakerNames.put({ meetingId, names })`). Replaces the meeting's row.
///
/// `names` is the raw-id → assigned-name map (only renamed speakers). An empty
/// map deletes the row so a cleared meeting carries no stale labels.
///
/// # Errors
///
/// Returns [`StorageError`] if the write fails.
pub async fn save_speaker_names(meeting_id: u32, names: BTreeMap<String, String>) -> Result<()> {
    let db = open_db().await?;
    let tx = db
        .transaction([SPEAKER_NAMES_STORE])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store(SPEAKER_NAMES_STORE)?;

    if names.is_empty() {
        store
            .delete::<u32, _>(meeting_id)
            .build()
            .map_err(|e| StorageError::Operation(format!("delete speakerNames: {e:?}")))?
            .await
            .map_err(|e| StorageError::Operation(format!("delete sn await: {e:?}")))?;
    } else {
        let row = SpeakerName { meeting_id, names };
        store
            .put(&row)
            .serde()
            .map_err(|e| StorageError::Operation(format!("put speakerNames: {e:?}")))?
            .await
            .map_err(|e| StorageError::Operation(format!("put sn await: {e:?}")))?;
    }
    tx.commit().await?;
    Ok(())
}

/// Load the per-meeting speaker rename map (`speakerNames.get(meetingId)`), or an
/// empty map if the meeting has none.
///
/// # Errors
///
/// Returns [`StorageError`] if the read fails.
pub async fn load_speaker_names(meeting_id: u32) -> Result<BTreeMap<String, String>> {
    let db = open_db().await?;
    let tx = db
        .transaction([SPEAKER_NAMES_STORE])
        .with_mode(TransactionMode::Readonly)
        .build()?;
    let store = tx.object_store(SPEAKER_NAMES_STORE)?;
    let row: Option<SpeakerName> = store
        .get::<SpeakerName, u32, _>(meeting_id)
        .serde()
        .map_err(|e| StorageError::Operation(format!("get speakerNames: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("get sn await: {e:?}")))?;
    tx.commit().await?;
    Ok(row.map(|r| r.names).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Extension grant sets (PRD Phase 6 / R7) — the persisted, revocable per-extension
// `GrantSet` the user approved at install. The GrantSet TYPE lives in
// `silent-extension-sdk`; this layer stores its serde JSON as an opaque string so
// storage never depends on the SDK (`docs/EXTENSIONS.md` §2: "recorded locally
// (IndexedDB, same store as meeting data)").
// ---------------------------------------------------------------------------

/// `extensionGrants.put({ extension, grant })`: persist one extension's grant set.
///
/// `extension` is the validated extension name (the store's primary key);
/// `grant_json` is the serde-serialized `GrantSet` as a JSON string. A re-grant
/// for the same extension overwrites the row.
///
/// # Errors
///
/// Returns [`StorageError`] if the write fails.
pub async fn save_extension_grant(extension: &str, grant_json: &str) -> Result<()> {
    let db = open_db().await?;
    let tx = db
        .transaction([EXTENSION_GRANTS_STORE])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store(EXTENSION_GRANTS_STORE)?;

    let obj = Object::new();
    set(&obj, "extension", &JsValue::from_str(extension))?;
    set(&obj, "grant", &JsValue::from_str(grant_json))?;
    store
        .put(&JsValue::from(obj))
        .build()
        .map_err(|e| StorageError::Operation(format!("put extensionGrants: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("put eg await: {e:?}")))?;

    tx.commit().await?;
    Ok(())
}

/// `extensionGrants.get(extension)`: load one extension's grant-set JSON, or
/// `None` if the extension is not installed.
///
/// # Errors
///
/// Returns [`StorageError`] if the read fails.
pub async fn load_extension_grant(extension: &str) -> Result<Option<String>> {
    let db = open_db().await?;
    let tx = db
        .transaction([EXTENSION_GRANTS_STORE])
        .with_mode(TransactionMode::Readonly)
        .build()?;
    let store = tx.object_store(EXTENSION_GRANTS_STORE)?;
    let row: Option<JsValue> = store
        .get::<JsValue, String, _>(extension.to_owned())
        .primitive()
        .map_err(|e| StorageError::Operation(format!("get extensionGrants: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("get eg await: {e:?}")))?;
    tx.commit().await?;
    Ok(row.and_then(|v| grant_string(&v)))
}

/// `extensionGrants.toArray()`: load every installed extension's grant-set JSON,
/// for the extension-manager listing and the boot-time host re-hydration.
///
/// # Errors
///
/// Returns [`StorageError`] if the read fails.
pub async fn load_all_extension_grants() -> Result<Vec<String>> {
    let db = open_db().await?;
    let tx = db
        .transaction([EXTENSION_GRANTS_STORE])
        .with_mode(TransactionMode::Readonly)
        .build()?;
    let store = tx.object_store(EXTENSION_GRANTS_STORE)?;
    let rows = store
        .get_all::<JsValue>()
        .primitive()
        .map_err(|e| StorageError::Operation(format!("getAll extensionGrants: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("getAll eg await: {e:?}")))?;
    tx.commit().await?;
    // `get_all().primitive()` yields an iterator of `Result<JsValue, _>`; project
    // each successfully-decoded row to its `grant` JSON string (skipping any row
    // that failed to decode or is missing the field — a corrupt row never crashes
    // the listing).
    Ok(rows
        .filter_map(|r| r.ok().as_ref().and_then(grant_string))
        .collect())
}

/// `extensionGrants.delete(extension)`: remove one extension's grant set (the
/// full "remove extension" / revoke-all path).
///
/// # Errors
///
/// Returns [`StorageError`] if the delete fails.
pub async fn delete_extension_grant(extension: &str) -> Result<()> {
    let db = open_db().await?;
    let tx = db
        .transaction([EXTENSION_GRANTS_STORE])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store(EXTENSION_GRANTS_STORE)?;
    store
        .delete::<String, _>(extension.to_owned())
        .build()
        .map_err(|e| StorageError::Operation(format!("delete extensionGrants: {e:?}")))?
        .await
        .map_err(|e| StorageError::Operation(format!("delete eg await: {e:?}")))?;
    tx.commit().await?;
    Ok(())
}

/// Read the `grant` JSON string field out of one stored `extensionGrants` row.
/// A row missing/non-string `grant` is skipped (a corrupt row never crashes the
/// listing — it is simply not surfaced).
fn grant_string(row: &JsValue) -> Option<String> {
    Reflect::get(row, &JsValue::from_str("grant"))
        .ok()
        .and_then(|g| g.as_string())
}
