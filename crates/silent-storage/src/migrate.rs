//! The Dexie v2 → Rust zero-loss migration (wasm32 only).
//!
//! Sequence (S4 "Recommended Migration Strategy"):
//!
//! 1. Open the DB version-less and assert IDB version 20 (Dexie v2 pre-flight).
//! 2. Read the full [`StorageSnapshot`] with the two-phase reader.
//! 3. Build an export-backup and emit
//!    [`silent_core::storage::StorageEvent::BackupReady`] BEFORE any write — the
//!    UI offers it as a download (PRD Phase 4 exit criterion / Key Risk:
//!    "export-backup before migrate"). The migration does not write until this is
//!    produced.
//! 4. Normalize every screenshot's `image` to a `Uint8Array` in place (base64
//!    strings keep their exact bytes; Blobs become their resolved bytes), so
//!    post-migration reads are single-phase.
//! 5. Re-read and verify zero loss (counts + per-row content) against the
//!    pre-migration snapshot before declaring success.
//! 6. Record completion in `localStorage` so a second run is a no-op
//!    ([`MigrationStatus::AlreadyMigrated`]).
//!
//! Events are delivered to the UI through a caller-supplied `emit` callback (a
//! `&dyn Fn(StorageEvent)`), so this module needs no opinion about how the UI
//! renders them — it owns the POLICY (when to back up, when to write, how to
//! verify), the JS host owns the rendering (PRD R2: Rust owns the law).

use indexed_db_futures::database::Database;
use indexed_db_futures::prelude::*;
use indexed_db_futures::transaction::TransactionMode;
use js_sys::{Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use silent_core::storage::{
    ImageEncoding, MigrationStatus, Screenshot, StorageCounts, StorageEvent, StorageSnapshot,
};

use crate::backup::Backup;
use crate::error::{Result, StorageError};
use crate::reader;

/// `localStorage` key recording that the migration completed (so it runs once).
pub const MIGRATION_DONE_KEY: &str = "silentNotetaker_migrated_v3";

/// Run the full migration, emitting [`StorageEvent`]s through `emit`.
///
/// Returns the before/after [`StorageCounts`] on success. If the DB is already
/// migrated, returns the current counts with an
/// [`StorageEvent::StatusChanged`]`(AlreadyMigrated)` and writes nothing.
///
/// # Errors
///
/// Returns [`StorageError`] (and emits [`StorageEvent::Failed`]) on any failure;
/// no destructive write is committed past a failure point.
pub async fn run_migration(emit: &dyn Fn(StorageEvent)) -> Result<StorageCounts> {
    match run_migration_inner(emit).await {
        Ok(counts) => Ok(counts),
        Err(e) => {
            emit(StorageEvent::Failed {
                message: e.to_string(),
            });
            Err(e)
        }
    }
}

async fn run_migration_inner(emit: &dyn Fn(StorageEvent)) -> Result<StorageCounts> {
    // ── Already migrated? ────────────────────────────────────────────────────
    if migration_already_done()? {
        let db = reader::open().await?;
        let snapshot = reader::read_all(&db).await?;
        let counts = snapshot.counts();
        emit(StorageEvent::StatusChanged {
            status: MigrationStatus::AlreadyMigrated,
        });
        return Ok(counts);
    }

    // ── 1. Open + pre-flight ─────────────────────────────────────────────────
    let db = reader::open().await?;
    reader::assert_dexie_v2(&db)?;

    // ── 2. Read everything ───────────────────────────────────────────────────
    let snapshot = reader::read_all(&db).await?;
    let before = snapshot.counts();

    // ── 3. Export-backup BEFORE any write ────────────────────────────────────
    emit(StorageEvent::StatusChanged {
        status: MigrationStatus::AwaitingBackup,
    });
    emit_backup_ready(&snapshot, before, emit)?;

    // ── 4. Normalize screenshots to Uint8Array ───────────────────────────────
    emit(StorageEvent::StatusChanged {
        status: MigrationStatus::Migrating,
    });
    normalize_screenshots(&db, &snapshot.screenshots, emit).await?;

    // ── 5. Verify zero loss ──────────────────────────────────────────────────
    let after_snapshot = reader::read_all(&db).await?;
    let after = after_snapshot.counts();
    verify_zero_loss(&snapshot, &after_snapshot)?;

    // ── 6. Record completion ─────────────────────────────────────────────────
    mark_migration_done()?;
    emit(StorageEvent::StatusChanged {
        status: MigrationStatus::Complete,
    });
    emit(StorageEvent::Completed { before, after });

    Ok(after)
}

/// Build the backup, hand its bytes to a Blob + object URL, and emit
/// [`StorageEvent::BackupReady`]. The UI links `object_url` for download.
fn emit_backup_ready(
    snapshot: &StorageSnapshot,
    counts: StorageCounts,
    emit: &dyn Fn(StorageEvent),
) -> Result<()> {
    let backup = Backup::from_snapshot(snapshot);
    let bytes = backup.to_json_bytes().map_err(StorageError::Backup)?;

    let now = js_sys::Date::now();
    let filename = Backup::filename(now);
    let (object_url, size_bytes) = make_object_url(&bytes)?;

    emit(StorageEvent::BackupReady {
        object_url,
        filename,
        size_bytes,
        counts,
    });
    Ok(())
}

/// Create a `blob:` object URL for the backup bytes (UI downloads via an `<a>`).
fn make_object_url(bytes: &[u8]) -> Result<(String, u32)> {
    let arr = js_sys::Array::new();
    arr.push(&Uint8Array::from(bytes).into());

    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/json");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&arr, &opts)
        .map_err(|e| StorageError::Js(format!("Blob::new: {e:?}")))?;

    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| StorageError::Js(format!("createObjectURL: {e:?}")))?;

    // Backup size is bounded by realistic meeting data; u32 is ample, and the
    // cast is from a known-non-negative length.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "backup byte length fits u32 for any realistic meeting database"
    )]
    let size = bytes.len() as u32;
    Ok((url, size))
}

/// Re-store every screenshot's `image` as a `Uint8Array` (the normalized layout)
/// in a single read-write transaction, emitting [`StorageEvent::Progress`].
///
/// The bytes written are exactly the bytes the reader materialized (base64
/// strings → their UTF-8 bytes; Blobs → their resolved bytes), so the payload is
/// preserved; only the JS storage TYPE changes. After this, every screenshot is
/// `ImageEncoding::Bytes` and reads are single-phase.
async fn normalize_screenshots(
    db: &Database,
    screenshots: &[Screenshot],
    emit: &dyn Fn(StorageEvent),
) -> Result<()> {
    // Total rows to normalize (already-`Bytes` rows are skipped but still
    // counted toward progress so the UI sees a complete bar).
    #[allow(
        clippy::cast_possible_truncation,
        reason = "screenshot count fits u32 for any realistic meeting database"
    )]
    let total = screenshots.len() as u32;

    let tx = db
        .transaction(["screenshots"])
        .with_mode(TransactionMode::Readwrite)
        .build()?;
    let store = tx.object_store("screenshots")?;

    for (idx, s) in screenshots.iter().enumerate() {
        // A row already stored as raw bytes needs no rewrite.
        if s.image_encoding != Some(ImageEncoding::Bytes) {
            let record = screenshot_to_js(s)?;
            store
                .put(&record)
                .build()
                .map_err(|e| StorageError::Operation(format!("put: {e:?}")))?
                .await
                .map_err(|e| StorageError::Operation(format!("put await: {e:?}")))?;
        }
        // `idx + 1` rows processed so far; bounded by the screenshot count.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "screenshot count fits u32 for any realistic meeting database"
        )]
        let done = (idx + 1) as u32;
        emit(StorageEvent::Progress { done, total });
    }

    tx.commit().await?;
    Ok(())
}

/// Build the JS object IDB stores for a normalized screenshot: every field from
/// the schema, with `image` as a `Uint8Array`, plus an `imageEncoding` marker
/// recording the ORIGINAL encoding.
///
/// The marker resolves an otherwise-real ambiguity: after normalization every
/// `image` is a `Uint8Array`, so a reader could not tell a screenshot that was
/// originally a base64 data-URL STRING (whose bytes are the UTF-8 of that string,
/// recovered with `String::from_utf8`) from one that was originally binary (real
/// JPEG/PNG bytes). The `imageEncoding` marker (`base64` / `blob` / `bytes`)
/// preserves the original representation losslessly so the Rust-owned render path
/// reconstructs the correct `<img src>` (a data-URL string, or an object URL over
/// the binary). `imageEncoding` is a non-indexed extra property — Dexie/IDB store
/// it freely without a schema change.
fn screenshot_to_js(s: &Screenshot) -> Result<JsValue> {
    let obj = Object::new();
    let set = |key: &str, val: &JsValue| -> Result<()> {
        Reflect::set(&obj, &JsValue::from_str(key), val)
            .map(|_| ())
            .map_err(|e| StorageError::Js(format!("Reflect::set({key}): {e:?}")))
    };

    // u32 → f64 is lossless (u32 < 2^53); IDB stores numbers as f64.
    set("id", &JsValue::from_f64(f64::from(s.id)))?;
    set("meetingId", &JsValue::from_f64(f64::from(s.meeting_id)))?;
    set("timestamp", &JsValue::from_f64(s.timestamp))?;
    set("image", &Uint8Array::from(s.image.as_slice()).into())?;
    set("width", &JsValue::from_f64(f64::from(s.width)))?;
    set("height", &JsValue::from_f64(f64::from(s.height)))?;
    set("analyzed", &JsValue::from_bool(s.analyzed))?;
    set("analysis", &JsValue::from_str(&s.analysis))?;
    // Record the ORIGINAL encoding (default `bytes` if a row was already binary).
    let original = s.image_encoding.unwrap_or(ImageEncoding::Bytes);
    set("imageEncoding", &JsValue::from_str(original.as_str()))?;
    Ok(obj.into())
}

/// Verify the post-migration DB matches the pre-migration snapshot with zero
/// loss: equal row counts, equal scalar content, byte-identical screenshot
/// payloads, AND preserved original encoding (the bytes move to a `Uint8Array`
/// but the `imageEncoding` marker keeps each row's original representation).
///
/// # Errors
///
/// Returns [`StorageError::ZeroLoss`] describing the first mismatch found.
fn verify_zero_loss(before: &StorageSnapshot, after: &StorageSnapshot) -> Result<()> {
    if before.meetings != after.meetings {
        return Err(StorageError::ZeroLoss(
            "meetings differ after migration".into(),
        ));
    }
    if before.transcript_chunks != after.transcript_chunks {
        return Err(StorageError::ZeroLoss(
            "transcriptChunks differ after migration".into(),
        ));
    }
    if before.notes != after.notes {
        return Err(StorageError::ZeroLoss(
            "notes differ after migration".into(),
        ));
    }
    if before.screenshots.len() != after.screenshots.len() {
        return Err(StorageError::ZeroLoss(format!(
            "screenshot count changed: {} → {}",
            before.screenshots.len(),
            after.screenshots.len()
        )));
    }
    for (b, a) in before.screenshots.iter().zip(&after.screenshots) {
        if b.id != a.id
            || b.meeting_id != a.meeting_id
            || b.timestamp.to_bits() != a.timestamp.to_bits()
            || b.width != a.width
            || b.height != a.height
            || b.analyzed != a.analyzed
            || b.analysis != a.analysis
        {
            return Err(StorageError::ZeroLoss(format!(
                "screenshot {} metadata changed after migration",
                b.id
            )));
        }
        if b.image != a.image {
            return Err(StorageError::ZeroLoss(format!(
                "screenshot {} image bytes changed after migration ({} → {} bytes)",
                b.id,
                b.image.len(),
                a.image.len()
            )));
        }
        // The ORIGINAL encoding must survive: the migration stores the bytes as a
        // `Uint8Array` PLUS an `imageEncoding` marker, and the reader recovers the
        // marker — so a base64 row reads back as `Base64String`, a Blob row as
        // `Blob`, etc. Preserving the encoding is what lets the render path
        // reconstruct the correct representation (data-URL vs object-URL).
        if b.image_encoding != a.image_encoding {
            return Err(StorageError::ZeroLoss(format!(
                "screenshot {} encoding changed after migration ({:?} → {:?})",
                b.id, b.image_encoding, a.image_encoding
            )));
        }
    }
    Ok(())
}

fn local_storage() -> Result<web_sys::Storage> {
    let win = web_sys::window().ok_or_else(|| StorageError::Js("no window".into()))?;
    win.local_storage()
        .map_err(|e| StorageError::Js(format!("local_storage(): {e:?}")))?
        .ok_or_else(|| StorageError::Js("localStorage unavailable".into()))
}

fn migration_already_done() -> Result<bool> {
    let ls = local_storage()?;
    let val = ls
        .get_item(MIGRATION_DONE_KEY)
        .map_err(|e| StorageError::Js(format!("getItem: {e:?}")))?;
    Ok(val.as_deref() == Some("1"))
}

fn mark_migration_done() -> Result<()> {
    let ls = local_storage()?;
    ls.set_item(MIGRATION_DONE_KEY, "1")
        .map_err(|e| StorageError::Js(format!("setItem: {e:?}")))
}
