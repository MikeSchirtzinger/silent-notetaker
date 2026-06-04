//! The browser IndexedDB reader (wasm32 only) — productionized from the S4
//! spike (`docs/research/spike-storage.md`).
//!
//! Opens the Dexie v2 `SilentNotetaker` database WITHOUT requesting a version
//! (the Dexie ×10 rule, [`silent_core::storage::DEXIE_VERSION_MULTIPLIER`]) and
//! reads all four tables into a [`StorageSnapshot`]. The screenshots table uses
//! the proven TWO-PHASE read so a JS `Blob` payload (whose bytes are only
//! available via the async `arrayBuffer()` Promise) can be resolved without
//! holding an IDB transaction across a non-IDB `await` — the universal IDB
//! transaction-auto-close trap, confirmed empirically in the spike.

use futures::TryStreamExt as _;
use indexed_db_futures::database::Database;
use indexed_db_futures::object_store::ObjectStore;
use indexed_db_futures::prelude::*;
use indexed_db_futures::transaction::TransactionMode;
use js_sys::{Reflect, Uint8Array};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use silent_core::storage::{
    ImageEncoding, Meeting, Note, Screenshot, StorageSnapshot, TranscriptChunk,
    expected_idb_version,
};

use crate::error::{Result, StorageError};

/// The database name the shipping app uses (`new Dexie('SilentNotetaker')`).
pub const DB_NAME: &str = "SilentNotetaker";

/// Open the `SilentNotetaker` database at its CURRENT version.
///
/// No `with_version` / no upgrade callback: Dexie created the DB at IDB version
/// 20 (`db.version(2) × 10`), and requesting any specific version from Rust would
/// trigger a spurious version-change. Opening version-less works across any Dexie
/// minor version (S4 Key Finding 1 / Gap 4).
///
/// # Errors
///
/// Returns [`StorageError::Open`] if the database cannot be opened.
pub async fn open() -> Result<Database> {
    Database::open(DB_NAME)
        .build()
        .map_err(|e| StorageError::Open(format!("{e:?}")))?
        .await
        .map_err(|e| StorageError::Open(format!("{e:?}")))
}

/// Assert the opened DB is at the IDB version a Dexie v2 schema maps to (20).
///
/// The migration runs this pre-flight before any write so it refuses to touch a
/// database it does not recognize.
///
/// # Errors
///
/// Returns [`StorageError::UnexpectedVersion`] if the version is not `20`.
pub fn assert_dexie_v2(db: &Database) -> Result<()> {
    let expected = expected_idb_version(silent_core::storage::DEXIE_SCHEMA_VERSION);
    // `db.version()` is an f64 in indexed_db_futures; IDB versions are small
    // whole numbers well within exact-integer range.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "IDB version is a small non-negative whole number"
    )]
    let actual = db.version() as u32;
    if actual == expected {
        Ok(())
    } else {
        Err(StorageError::UnexpectedVersion { expected, actual })
    }
}

/// Read all four tables into a [`StorageSnapshot`], resolving every screenshot
/// payload (base64 string / `Blob` / `Uint8Array`) to raw bytes with zero loss.
///
/// # Errors
///
/// Returns [`StorageError`] on open, transaction, deserialize, or Blob-resolution
/// failure. A row that fails to deserialize is an ERROR, never a silent drop.
pub async fn read_all(db: &Database) -> Result<StorageSnapshot> {
    // Read each store in its own transaction (a transaction must not be held
    // across the others' awaits anyway). Built field-by-field rather than
    // mutating a `default()` so the await order is explicit and clippy-clean.
    let meetings = read_table_serde::<Meeting>(db, "meetings").await?;
    let transcript_chunks = read_table_serde::<TranscriptChunk>(db, "transcriptChunks").await?;
    let notes = read_table_serde::<Note>(db, "notes").await?;
    let screenshots = read_screenshots_two_phase(db).await?;

    Ok(StorageSnapshot {
        meetings,
        transcript_chunks,
        notes,
        screenshots,
    })
}

/// Open the DB and read everything in one call (the common entry point).
///
/// # Errors
///
/// See [`read_all`].
pub async fn read_database() -> Result<StorageSnapshot> {
    let db = open().await?;
    read_all(&db).await
}

/// Read every record of a fully-serde-deserializable store (no binary fields)
/// via a cursor.
///
/// # Errors
///
/// Returns [`StorageError`] if the cursor fails or any row fails to deserialize.
async fn read_table_serde<T>(db: &Database, store_name: &str) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let tx = db
        .transaction([store_name])
        .with_mode(TransactionMode::Readonly)
        .build()?;
    let store = tx.object_store(store_name)?;
    let records = read_all_serde(&store).await?;
    tx.commit().await?;
    Ok(records)
}

async fn read_all_serde<T>(store: &ObjectStore<'_>) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let mut records = Vec::new();
    // `open_cursor()` yields `None` for an empty store; the loop ends at once.
    if let Some(cursor) = store.open_cursor().build()?.await? {
        let mut stream = cursor.stream_ser::<T>();
        while let Some(value) = stream.try_next().await? {
            records.push(value);
        }
    }
    Ok(records)
}

/// A screenshot whose scalars are read but whose `image` may still need async
/// resolution (the `Blob` case). Phase-1 product; `pending_blob` holds a live JS
/// `Blob` handle for Phase 2 AFTER the transaction commits.
struct PartialScreenshot {
    id: u32,
    meeting_id: u32,
    timestamp: f64,
    width: u32,
    height: u32,
    analyzed: bool,
    analysis: String,
    /// Bytes resolved synchronously (base64 string / `Uint8Array` / empty).
    image: Vec<u8>,
    encoding: ImageEncoding,
    /// A live `Blob` handle to resolve in Phase 2; `None` for non-Blob encodings.
    pending_blob: Option<web_sys::Blob>,
}

/// The two-phase screenshot reader — the migration-critical path.
///
/// Phase 1 holds the transaction and extracts everything readable synchronously,
/// stashing live `Blob` handles. Phase 2 runs AFTER the transaction commits and
/// awaits `blob.arrayBuffer()` for each stashed `Blob`. A `Blob` handle stays
/// valid after its transaction closes (it is an independent JS object), which is
/// what makes this safe. We NEVER await a non-IDB future while a transaction is
/// open (S4 Key Finding 3 / Gap 6).
///
/// # Errors
///
/// Returns [`StorageError`] on cursor failure, a missing/mistyped scalar field,
/// or `Blob` resolution failure.
async fn read_screenshots_two_phase(db: &Database) -> Result<Vec<Screenshot>> {
    // ── Phase 1: transaction held, no non-IDB awaits ─────────────────────────
    let mut partials: Vec<PartialScreenshot> = Vec::new();
    {
        let tx = db
            .transaction(["screenshots"])
            .with_mode(TransactionMode::Readonly)
            .build()?;
        let store = tx.object_store("screenshots")?;

        if let Some(cursor) = store.open_cursor().build()?.await? {
            let mut stream = cursor.stream::<JsValue>();
            // Only IDB cursor advances are awaited here — the tx stays alive.
            while let Some(raw) = stream.try_next().await? {
                partials.push(extract_partial_screenshot(&raw)?);
            }
        }

        // Commit explicitly; the tx closes here. Blob handles in `partials`
        // remain valid because they are independent JS objects.
        tx.commit().await?;
    }

    // ── Phase 2: NO transaction held — safe to await Blob promises ───────────
    let mut records = Vec::with_capacity(partials.len());
    for p in partials {
        let image = if let Some(blob) = p.pending_blob {
            let ab = wasm_bindgen_futures::JsFuture::from(blob.array_buffer())
                .await
                .map_err(|e| StorageError::Js(format!("Blob.arrayBuffer() rejected: {e:?}")))?;
            Uint8Array::new(&ab).to_vec()
        } else {
            p.image
        };

        records.push(Screenshot {
            id: p.id,
            meeting_id: p.meeting_id,
            timestamp: p.timestamp,
            image,
            image_encoding: Some(p.encoding),
            width: p.width,
            height: p.height,
            analyzed: p.analyzed,
            analysis: p.analysis,
        });
    }

    Ok(records)
}

/// Phase-1 extraction: scalar fields + classify the `image` encoding.
///
/// Resolves base64-string, bytes, and empty immediately; for a `Blob`, stashes
/// the live handle for Phase-2 async resolution. Does NOT await anything (we are
/// inside the open transaction).
///
/// # Errors
///
/// Returns [`StorageError::Deserialize`] if a required scalar is missing/mistyped.
fn extract_partial_screenshot(raw: &JsValue) -> Result<PartialScreenshot> {
    // IDB auto-increment keys and pixel dims fit u32; f64→u32 is exact here.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "IDB ids and pixel dimensions are small non-negative whole numbers"
    )]
    let to_u32 = |v: f64| v as u32;

    let get = |key: &str| -> Result<JsValue> {
        Reflect::get(raw, &JsValue::from_str(key))
            .map_err(|e| StorageError::Js(format!("Reflect::get({key}): {e:?}")))
    };

    let id = get("id")?
        .as_f64()
        .map(to_u32)
        .ok_or_else(|| StorageError::Deserialize("screenshots.id not a number".into()))?;

    let meeting_id = get("meetingId")?
        .as_f64()
        .map(to_u32)
        .ok_or_else(|| StorageError::Deserialize("screenshots.meetingId not a number".into()))?;

    let timestamp = get("timestamp")?
        .as_f64()
        .ok_or_else(|| StorageError::Deserialize("screenshots.timestamp not a number".into()))?;

    let width = get("width")?
        .as_f64()
        .map(to_u32)
        .ok_or_else(|| StorageError::Deserialize("screenshots.width not a number".into()))?;

    let height = get("height")?
        .as_f64()
        .map(to_u32)
        .ok_or_else(|| StorageError::Deserialize("screenshots.height not a number".into()))?;

    let analyzed = get("analyzed")?.is_truthy();
    let analysis = get("analysis")?.as_string().unwrap_or_default();

    // The migration writes an `imageEncoding` marker recording the ORIGINAL
    // encoding (see `migrate::screenshot_to_js`). A POST-migration row is always
    // a `Uint8Array`; the marker tells us whether those bytes were originally a
    // base64 data-URL string (recover with `String::from_utf8`), a Blob, or
    // already binary — so a migrated DB round-trips its original encoding exactly.
    let original_marker = get("imageEncoding")
        .ok()
        .and_then(|v| v.as_string())
        .and_then(|s| ImageEncoding::from_marker(&s));

    // ── Classify the image encoding ──────────────────────────────────────────
    let image_val = get("image")?;
    let (image, encoding, pending_blob) = if let Some(s) = image_val.as_string() {
        // CURRENT app layout: base64 data-URL string. Preserve the exact bytes
        // of the string so readback is byte-identical to what `<img src>` reads.
        (s.into_bytes(), ImageEncoding::Base64String, None)
    } else if image_val.is_instance_of::<Uint8Array>()
        || image_val.is_instance_of::<js_sys::ArrayBuffer>()
    {
        // Binary layout (raw, or the format the migration normalizes TO). If an
        // `imageEncoding` marker is present it carries the ORIGINAL encoding
        // (a migrated row); otherwise this is a natively-binary row (`Bytes`).
        let bytes = Uint8Array::new(&image_val).to_vec();
        (bytes, original_marker.unwrap_or(ImageEncoding::Bytes), None)
    } else if image_val.is_instance_of::<web_sys::Blob>() {
        // MIGRATION-CRITICAL: stash the live Blob handle; resolve in Phase 2.
        let blob: web_sys::Blob = image_val.unchecked_into();
        (Vec::new(), ImageEncoding::Blob, Some(blob))
    } else {
        // Null/undefined/absent — screenshot slot not populated.
        (Vec::new(), ImageEncoding::Empty, None)
    };

    Ok(PartialScreenshot {
        id,
        meeting_id,
        timestamp,
        width,
        height,
        analyzed,
        analysis,
        image,
        encoding,
        pending_blob,
    })
}
