//! Storage domain contracts: the Dexie v2 `SilentNotetaker` schema as typed Rust
//! records, plus the migration/backup boundary the UI drives (PRD Phase 4,
//! Appendix A rows 1–3, 17, 26, 29, 33).
//!
//! These are the SINGLE source of truth for the four tables
//! (`meetings`, `transcriptChunks`, `notes`, `screenshots`). The browser read /
//! migrate implementation lives in `silent-storage` (which needs `web-sys` and
//! cannot live here — `silent-core` must compile for `wasm32-unknown-unknown`
//! with no browser deps); this module owns the *shapes* and the *events*.
//!
//! # Schema (index.html ~line 1975, Dexie v2)
//!
//! ```javascript
//! db.version(2).stores({
//!   meetings:         '++id, title, startTime, endTime, duration',
//!   transcriptChunks: '++id, meetingId, timestamp, text, isFinal',
//!   notes:            '++id, meetingId, category, text, timestamp, triggerPhrase',
//!   screenshots:      '++id, meetingId, timestamp, image, width, height, analyzed, analysis',
//! });
//! ```
//!
//! # Schema v3 (Rust-owned, this task)
//!
//! When Dexie is removed and `silent-storage` becomes the sole owner of the
//! `SilentNotetaker` database, the schema gains one store and the four existing
//! stores keep their exact key paths so the live CRUD and the zero-loss migration
//! both keep working:
//!
//! ```text
//! speakerNames: keyPath "meetingId" (NO autoIncrement) — the durable
//!               per-meeting speaker-rename map (Phase-F carry-forward): the
//!               human labels the user assigned to `S1`/`S2`/… so renames survive
//!               a reload. One row per meeting; the row's `names` object maps a
//!               raw speaker id to its assigned name.
//! ```
//!
//! Bumping the Dexie user-version `2` → `3` raises the raw IDB version `20` → `30`
//! (the ×10 rule), and the upgrade creates `speakerNames` if absent. The four
//! Dexie v2 stores are created (for a fresh install) or left untouched (existing
//! DBs), so a v2 database opened at v3 is non-destructive — exactly what the
//! migration assumes.
//!
//! # Faithfulness to real captured data (not the synthetic spike fixture)
//!
//! The structs encode what the SHIPPING app actually writes, including the
//! partially-populated rows a real DB contains:
//!
//! - A meeting in progress (or one whose tab closed before Stop) is written with
//!   `endTime: null, duration: 0` (index.html:3969). `end_time` is therefore
//!   `Option<f64>` — a non-optional `f64` would FAIL to deserialize `null` and
//!   lose that user's meeting. This is the exact failure the spike's synthetic
//!   fixture (which always set `endTime`) could not surface.
//! - A screenshot is added with `analyzed: false` and NO `analysis` field
//!   (index.html:2817); `analysis` is filled in later by the Claude bridge.
//!   `analysis` is `#[serde(default)]` so an absent field reads as `""`.
//! - Older note rows or AI-final notes may omit `triggerPhrase`;
//!   `#[serde(default)]` keeps them rather than dropping the row.
//!
//! Tolerant deserialization here is a correctness requirement: the migration's
//! whole job is that *existing users' meetings survive the upgrade* (PRD Phase 4
//! exit criterion / Key Risk: "Dexie→Rust storage migration loses meetings").
//!
//! # TypeScript boundary
//!
//! Like every other boundary type, these derive [`ts_rs::TS`] under `#[cfg(test)]`
//! and export to `crates/silent-core/bindings/`. The binary screenshot payload
//! (`Screenshot::image`) is `#[serde(skip)]` — raw image bytes do not cross the
//! JSON boundary; the UI offers the backup blob via [`StorageEvent::BackupReady`]
//! as an opaque object URL, not inline bytes.

use serde::{Deserialize, Serialize};

/// A `meetings` row.
///
/// Dexie schema: `++id, title, startTime, endTime, duration`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Meeting {
    /// Auto-incrementing primary key (Dexie `++id`).
    pub id: u32,
    /// Human-readable title (UI caps input at 120 chars; the stored value is
    /// trusted as-is — a longer legacy value is preserved, not truncated).
    pub title: String,
    /// Unix epoch milliseconds at recording start.
    #[serde(rename = "startTime")]
    pub start_time: f64,
    /// Unix epoch milliseconds at recording end, or `None` for a meeting that
    /// never reached Stop (`endTime: null` in the shipping app, index.html:3972).
    ///
    /// `default` accepts a row whose `endTime` key is absent entirely; it
    /// serializes back as `endTime: null` (not omitted) so a re-stored backup is
    /// byte-faithful to what Dexie wrote. `#[ts(rename)]` is given explicitly:
    /// ts-rs's serde-compat does not pick up `#[serde(rename)]` when the field
    /// also carries `default` on an `Option`, so the TS key would otherwise drift
    /// to `end_time` and break the UI typing.
    #[serde(rename = "endTime", default)]
    #[cfg_attr(test, ts(rename = "endTime"))]
    pub end_time: Option<f64>,
    /// Duration in milliseconds (`0` until the meeting is stopped).
    pub duration: f64,
}

/// A `transcriptChunks` row.
///
/// Dexie schema: `++id, meetingId, timestamp, text, isFinal`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct TranscriptChunk {
    /// Auto-incrementing primary key.
    pub id: u32,
    /// Foreign key to `meetings.id`.
    #[serde(rename = "meetingId")]
    pub meeting_id: u32,
    /// Offset milliseconds from meeting start (`Date.now()` at write time).
    pub timestamp: f64,
    /// Transcript text for this chunk.
    pub text: String,
    /// `true` for a finalized (non-draft) chunk (index.html:4326 always writes
    /// `true`; drafts are not persisted, but the field is read tolerantly).
    #[serde(rename = "isFinal")]
    pub is_final: bool,
}

/// A `notes` row (a trigger note, AI note, or question).
///
/// Dexie schema: `++id, meetingId, category, text, timestamp, triggerPhrase`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Note {
    /// Auto-incrementing primary key.
    pub id: u32,
    /// Foreign key to `meetings.id`.
    #[serde(rename = "meetingId")]
    pub meeting_id: u32,
    /// Category as the app writes it (`decisions`, `actions`, `keypoints`,
    /// `questions` — see index.html:3810/2660). Kept as a free string so a future
    /// category is preserved, not rejected.
    pub category: String,
    /// Note text.
    pub text: String,
    /// Offset milliseconds from meeting start.
    pub timestamp: f64,
    /// The phrase that triggered detection. `#[serde(default)]` because AI-final
    /// notes and older rows may omit it (the trigger path always sets it,
    /// index.html:2662, but the migration must not lose a note that lacks it).
    #[serde(rename = "triggerPhrase", default)]
    pub trigger_phrase: String,
}

/// How the `image` field of a screenshot was stored in `IndexedDB`, discovered
/// at read time.
///
/// Real `SilentNotetaker` data uses different layouts depending on the app
/// version and capture path. All three are proven zero-loss in the S4 spike
/// (`docs/research/spike-storage.md`):
///
/// - [`ImageEncoding::Base64String`] — the CURRENT shipping path: `canvas.toBlob()`
///   → `FileReader.readAsDataURL()` → a `data:image/jpeg;base64,...` STRING
///   (index.html:2813/2820). This is what every live DB contains today.
/// - [`ImageEncoding::Blob`] — the migration-critical case. A canvas `Blob`
///   stored directly (a plausible alternate/future capture path, and what a naive
///   "store the `toBlob()` result" produces). Its bytes are only readable via the
///   async `blob.arrayBuffer()` Promise, which forces the two-phase IDB read.
/// - [`ImageEncoding::Bytes`] — a `Uint8Array`/`ArrayBuffer` stored directly: the
///   normalized layout the migration WRITES, so post-migration reads are
///   single-phase.
/// - [`ImageEncoding::Empty`] — the `image` field was null/undefined/absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImageEncoding {
    /// `data:<mime>;base64,...` string (current production layout).
    Base64String,
    /// A JS `Blob` resolved via async `arrayBuffer()` (migration-critical layout).
    Blob,
    /// A `Uint8Array` / `ArrayBuffer` read synchronously (the normalized layout).
    Bytes,
    /// `image` was null/undefined/absent.
    Empty,
}

impl ImageEncoding {
    /// The lowercase wire tag for this encoding (used in the wasm readback
    /// summary, the migration's stored `imageEncoding` marker, and migration
    /// reports so a harness can assert each layout was actually exercised).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ImageEncoding::Base64String => "base64",
            ImageEncoding::Blob => "blob",
            ImageEncoding::Bytes => "bytes",
            ImageEncoding::Empty => "empty",
        }
    }

    /// Parse a stored `imageEncoding` marker tag back into an [`ImageEncoding`].
    ///
    /// The exact inverse of [`ImageEncoding::as_str`]; the migration writes the
    /// tag and the reader parses it so a migrated row recovers its ORIGINAL
    /// encoding (a base64 row stays distinguishable from a natively-binary one
    /// even though both are stored as `Uint8Array` after normalization). An
    /// unknown tag returns `None` (the reader then falls back to structural
    /// classification).
    #[must_use]
    pub fn from_marker(tag: &str) -> Option<Self> {
        match tag {
            "base64" => Some(ImageEncoding::Base64String),
            "blob" => Some(ImageEncoding::Blob),
            "bytes" => Some(ImageEncoding::Bytes),
            "empty" => Some(ImageEncoding::Empty),
            _ => None,
        }
    }
}

/// A `screenshots` row.
///
/// Dexie schema:
/// `++id, meetingId, timestamp, image, width, height, analyzed, analysis`.
///
/// The `image` field is polymorphic in real data ([`ImageEncoding`]); the reader
/// always materializes the raw payload bytes into [`Screenshot::image`] and
/// records which encoding it found in [`Screenshot::image_encoding`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct Screenshot {
    /// Auto-incrementing primary key.
    pub id: u32,
    /// Foreign key to `meetings.id`.
    #[serde(rename = "meetingId")]
    pub meeting_id: u32,
    /// Capture time offset milliseconds.
    pub timestamp: f64,
    /// Raw image payload bytes, materialized by the reader.
    ///
    /// - For [`ImageEncoding::Base64String`]: the UTF-8 bytes of the FULL data-URL
    ///   string, so readback is byte-identical to the value `<img src>` reads
    ///   (the migration preserves the exact stored string).
    /// - For [`ImageEncoding::Blob`] / [`ImageEncoding::Bytes`]: the decoded binary
    ///   image bytes (JPEG/PNG).
    ///
    /// `#[serde(skip)]` — binary does not cross the JSON boundary; it is handled
    /// via JS interop and the [`StorageEvent::BackupReady`] blob.
    #[serde(skip)]
    pub image: Vec<u8>,
    /// Which storage encoding the `image` field used; `None` before a read has
    /// classified it. `#[serde(skip)]` — derived at read time, not stored in IDB.
    #[serde(skip)]
    pub image_encoding: Option<ImageEncoding>,
    /// Width of the captured frame in pixels.
    pub width: u32,
    /// Height of the captured frame in pixels.
    pub height: u32,
    /// Whether Claude-bridge analysis has run.
    pub analyzed: bool,
    /// Claude-bridge analysis text. `#[serde(default)]` because a fresh capture
    /// has no `analysis` field at all (index.html:2817 omits it).
    #[serde(default)]
    pub analysis: String,
}

/// A `speakerNames` row: the durable per-meeting speaker-rename map
/// (Phase-F carry-forward).
///
/// The diarization tracker holds the human labels the user assigns to raw
/// speaker ids (`S1`, `S2`, …) only in memory, so renames were lost on reload —
/// the Phase-F gap this record closes. One row per meeting, keyed by
/// `meeting_id` (NOT auto-increment): writing the same meeting's map again
/// `put`s over the row. `names` maps a raw speaker id to its assigned name; only
/// renamed speakers appear (an absent id falls back to the raw id, exactly as the
/// live tracker does).
///
/// This is additive schema (a new store, no change to the four existing ones),
/// so it never affects the zero-loss migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct SpeakerName {
    /// Foreign key to `meetings.id` AND the store's primary key (one row per
    /// meeting; `put` overwrites).
    #[serde(rename = "meetingId")]
    pub meeting_id: u32,
    /// Map of raw speaker id (`"S1"`) → assigned human name (`"Alice"`). Only
    /// renamed speakers are stored; an id not present here keeps its raw label.
    pub names: std::collections::BTreeMap<String, String>,
}

/// The `speakerNames` store name (schema v3, Rust-owned).
pub const SPEAKER_NAMES_STORE: &str = "speakerNames";

/// The `extensionGrants` store name (schema v4, Rust-owned; PRD Phase 6 / R7).
///
/// One row per installed extension, keyed by the extension `name` (a validated
/// `[a-z0-9._-]` id, safe as an IDB string key — NOT auto-increment). The row's
/// value is the serde-serialized `GrantSet` the user approved at install
/// (`docs/EXTENSIONS.md` §2 "recorded locally (IndexedDB, same store as meeting
/// data)"). The grant-set TYPE lives in `silent-extension-sdk`; storage persists
/// it as an opaque JSON string so this layer never depends on the SDK. Additive
/// schema (a new store), so it never touches the four migrated data stores.
pub const EXTENSION_GRANTS_STORE: &str = "extensionGrants";

/// The Rust-owned schema version that adds the `speakerNames` (v3) and
/// `extensionGrants` (v4) stores (`db.version(4)` equivalent; raw IDB version
/// `40` via the ×10 rule). Each is additive — opening a v2 or v3 database at v4
/// only creates the missing stores and never rewrites the migrated data stores.
pub const RUST_SCHEMA_VERSION: u32 = 4;

/// A complete readback of all four `SilentNotetaker` tables.
///
/// The zero-loss contract: `silent-storage` produces this from a real Dexie DB,
/// and a migration verifies the readback round-trips the source bit-for-bit.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StorageSnapshot {
    /// All rows from the `meetings` store.
    pub meetings: Vec<Meeting>,
    /// All rows from the `transcriptChunks` store.
    pub transcript_chunks: Vec<TranscriptChunk>,
    /// All rows from the `notes` store.
    pub notes: Vec<Note>,
    /// All rows from the `screenshots` store.
    pub screenshots: Vec<Screenshot>,
}

impl StorageSnapshot {
    /// Per-table row counts and total screenshot payload bytes — the cheap
    /// invariant a migration asserts before and after a normalization write.
    #[must_use]
    pub fn counts(&self) -> StorageCounts {
        StorageCounts {
            meetings: self.meetings.len(),
            transcript_chunks: self.transcript_chunks.len(),
            notes: self.notes.len(),
            screenshots: self.screenshots.len(),
            screenshot_bytes: self.screenshots.iter().map(|s| s.image.len()).sum(),
        }
    }
}

/// Row counts plus total screenshot payload bytes for a [`StorageSnapshot`].
///
/// A migration must preserve every count and the byte total across a
/// Blob→`Uint8Array` normalization (the bytes are re-encoded, not changed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct StorageCounts {
    /// `meetings` row count.
    pub meetings: usize,
    /// `transcriptChunks` row count.
    pub transcript_chunks: usize,
    /// `notes` row count.
    pub notes: usize,
    /// `screenshots` row count.
    pub screenshots: usize,
    /// Total bytes across all screenshot `image` payloads.
    pub screenshot_bytes: usize,
}

/// The Dexie ×10 version multiplier (verified in `dexie-open.ts` and empirically
/// against raw `IDBFactory` in the S4 spike).
///
/// Dexie calls `indexedDB.open(name, Math.round(verno * 10))`, so the
/// user-visible `db.version(2)` opens the database at IDB version **20**. The
/// reader opens WITHOUT requesting a version (no upgrade callback), which works
/// across any Dexie minor version; this constant exists for the pre-flight
/// assertion ([`expected_idb_version`]).
pub const DEXIE_VERSION_MULTIPLIER: u32 = 10;

/// The Dexie user-visible schema version the shipping app uses (`db.version(2)`).
pub const DEXIE_SCHEMA_VERSION: u32 = 2;

/// The raw `IndexedDB` version a Dexie user-version maps to (`dexie × 10`).
///
/// `expected_idb_version(2) == 20` — the value a migration pre-flight asserts
/// before touching the DB.
#[must_use]
pub fn expected_idb_version(dexie_version: u32) -> u32 {
    dexie_version * DEXIE_VERSION_MULTIPLIER
}

/// The state of a Dexie→Rust storage migration (core → UI).
///
/// `#[non_exhaustive]`: new phases can be added without breaking the UI, which
/// handles unknown variants with a wildcard arm (A3 escape hatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MigrationStatus {
    /// No migration has run; the DB is at the legacy Dexie v2 layout.
    Pending,
    /// A backup has been produced and offered to the user; the migration is
    /// waiting for the export-backup gate before any normalization write.
    AwaitingBackup,
    /// The Blob→`Uint8Array` normalization write is in progress.
    Migrating,
    /// The migration completed and the readback verified zero loss.
    Complete,
    /// The DB was already migrated (the completion marker was present); nothing
    /// to do.
    AlreadyMigrated,
}

/// A storage / migration event the core emits to the UI (core → UI).
///
/// `#[non_exhaustive]`, tagged `{ "tag": ..., "payload": ... }`, `snake_case` —
/// the same boundary layout as [`crate::commands::SessionEvent`] and
/// [`crate::diarization::DiarizationEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StorageEvent {
    /// The export-backup is ready BEFORE any migration write (PRD Phase 4 exit
    /// criterion / Key Risk mitigation: "export-backup before migrate"). The UI
    /// uses this to offer the user a download; `object_url` is a
    /// `URL.createObjectURL(blob)` for the backup file the migration produced.
    /// The migration does not proceed past [`MigrationStatus::AwaitingBackup`]
    /// until the UI confirms (or auto-confirms) it.
    BackupReady {
        /// `URL.createObjectURL` of the backup blob the UI links for download.
        object_url: String,
        /// Suggested download filename, for example
        /// `silent-notetaker-backup-<timestamp>.json`.
        filename: String,
        /// Backup size in bytes (for the UI to show and to sanity-check the blob).
        size_bytes: u32,
        /// The row counts captured in the backup, so the UI can show "backed up
        /// N meetings".
        counts: StorageCounts,
    },

    /// The migration changed phase. The UI reflects this in the status surface.
    StatusChanged {
        /// The new migration status.
        status: MigrationStatus,
    },

    /// Per-table progress during the normalization write (large DBs).
    Progress {
        /// Rows normalized so far (screenshots are the only normalized table).
        done: u32,
        /// Total rows to normalize.
        total: u32,
    },

    /// The migration finished. Carries the before/after counts so the UI (and
    /// tests) can assert zero loss without re-reading the DB.
    Completed {
        /// Row counts read from the source DB before migration.
        before: StorageCounts,
        /// Row counts read back after migration; must equal `before`.
        after: StorageCounts,
    },

    /// The migration failed and was rolled back (no destructive write was
    /// committed past the backup). The DB is left at its pre-migration state.
    Failed {
        /// Human-readable reason.
        message: String,
    },
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism (PRD lint config \
              allows this in tests)"
)]
mod tests {
    use super::*;

    #[test]
    fn meeting_serde_roundtrip_with_end_time() {
        let m = Meeting {
            id: 1,
            title: "Q1 Kickoff".into(),
            start_time: 1_700_000_000_000.0,
            end_time: Some(1_700_003_600_000.0),
            duration: 3_600_000.0,
        };
        let json = serde_json::to_string(&m).expect("serialize");
        assert!(json.contains("\"startTime\""), "camelCase key: {json}");
        let m2: Meeting = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, m2);
    }

    /// The production-critical case the synthetic spike fixture could NOT cover:
    /// a meeting written `endTime: null, duration: 0` (in-progress / crashed
    /// before Stop). A non-optional `f64` would fail here and lose the meeting.
    #[test]
    fn meeting_deserializes_null_end_time() {
        let raw = r#"{"id":7,"title":"In progress","startTime":1700000000000.0,"endTime":null,"duration":0}"#;
        let m: Meeting = serde_json::from_str(raw).expect("null endTime must deserialize");
        assert_eq!(m.id, 7);
        assert!(m.end_time.is_none(), "endTime: null → None");
        // `0.0` is exactly representable; compare bit patterns to satisfy the
        // float_cmp lint while asserting the exact stored value.
        assert_eq!(m.duration.to_bits(), 0.0_f64.to_bits());
    }

    /// Dexie can also omit `endTime` entirely (a never-stopped meeting in some
    /// app builds). `#[serde(default)]` must keep the row.
    #[test]
    fn meeting_deserializes_missing_end_time() {
        let raw = r#"{"id":8,"title":"No end key","startTime":1700000000000.0,"duration":0}"#;
        let m: Meeting = serde_json::from_str(raw).expect("missing endTime must deserialize");
        assert!(m.end_time.is_none());
    }

    #[test]
    fn transcript_chunk_serde_roundtrip() {
        let c = TranscriptChunk {
            id: 42,
            meeting_id: 1,
            timestamp: 5_000.0,
            text: "We decided to ship next week.".into(),
            is_final: true,
        };
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"meetingId\""));
        assert!(json.contains("\"isFinal\""));
        let c2: TranscriptChunk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, c2);
    }

    #[test]
    fn note_serde_roundtrip() {
        let n = Note {
            id: 7,
            meeting_id: 1,
            category: "decisions".into(),
            text: "We will use Rust for the backend".into(),
            timestamp: 12_000.0,
            trigger_phrase: "we decided".into(),
        };
        let json = serde_json::to_string(&n).expect("serialize");
        let n2: Note = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(n, n2);
    }

    /// AI-final notes / older rows may omit `triggerPhrase`; the row must be
    /// kept with an empty trigger, not dropped.
    #[test]
    fn note_deserializes_missing_trigger_phrase() {
        let raw = r#"{"id":9,"meetingId":1,"category":"keypoints","text":"AI summary point","timestamp":1.0}"#;
        let n: Note = serde_json::from_str(raw).expect("missing triggerPhrase must deserialize");
        assert_eq!(n.trigger_phrase, "");
        assert_eq!(n.category, "keypoints");
    }

    /// A fresh screenshot is written with `analyzed: false` and NO `analysis`
    /// field (index.html:2817). It must deserialize to `analysis == ""`.
    #[test]
    fn screenshot_deserializes_missing_analysis() {
        let raw = r#"{"id":3,"meetingId":1,"timestamp":15000.0,"width":1280,"height":720,"analyzed":false}"#;
        let s: Screenshot = serde_json::from_str(raw).expect("missing analysis must deserialize");
        assert_eq!(s.analysis, "");
        assert!(!s.analyzed);
        assert!(s.image.is_empty(), "image is serde(skip), defaults empty");
        assert!(s.image_encoding.is_none(), "encoding is serde(skip)");
    }

    /// The binary `image` payload and the derived `image_encoding` are
    /// `#[serde(skip)]`: they do not cross the JSON boundary.
    #[test]
    fn screenshot_image_is_skipped_in_serde() {
        let s = Screenshot {
            id: 3,
            meeting_id: 1,
            timestamp: 15_000.0,
            image: vec![0x89, 0x50, 0x4e, 0x47], // PNG magic
            image_encoding: Some(ImageEncoding::Blob),
            width: 1280,
            height: 720,
            analyzed: false,
            analysis: String::new(),
        };
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(!json.contains("0x89") && !json.contains("\"image\""));
        let s2: Screenshot = serde_json::from_str(&json).expect("deserialize");
        assert!(s2.image.is_empty());
        assert!(s2.image_encoding.is_none());
        assert_eq!(s.id, s2.id);
        assert_eq!(s.width, s2.width);
    }

    #[test]
    fn image_encoding_tags_are_stable() {
        assert_eq!(ImageEncoding::Base64String.as_str(), "base64");
        assert_eq!(ImageEncoding::Blob.as_str(), "blob");
        assert_eq!(ImageEncoding::Bytes.as_str(), "bytes");
        assert_eq!(ImageEncoding::Empty.as_str(), "empty");
        // serde uses snake_case variant names, which differ from the short wire
        // tags above; assert both so a rename of either is caught.
        let json = serde_json::to_string(&ImageEncoding::Base64String).expect("serialize");
        assert_eq!(json, "\"base64_string\"");
    }

    /// `from_marker` must be the exact inverse of `as_str` for every variant —
    /// the migration writes the tag and the reader parses it; a mismatch would
    /// silently mis-classify a migrated screenshot's original encoding.
    #[test]
    fn image_encoding_marker_round_trips() {
        for enc in [
            ImageEncoding::Base64String,
            ImageEncoding::Blob,
            ImageEncoding::Bytes,
            ImageEncoding::Empty,
        ] {
            assert_eq!(
                ImageEncoding::from_marker(enc.as_str()),
                Some(enc),
                "as_str↔from_marker must round-trip for {enc:?}"
            );
        }
        assert_eq!(ImageEncoding::from_marker("nonsense"), None);
    }

    #[test]
    fn dexie_version_multiplier_is_ten() {
        assert_eq!(DEXIE_VERSION_MULTIPLIER, 10);
        assert_eq!(expected_idb_version(DEXIE_SCHEMA_VERSION), 20);
        assert_eq!(expected_idb_version(2), 20);
    }

    /// The Rust-owned schema (v4) raises the raw IDB version to 40, leaving the
    /// Dexie v2 layout (20) intact for the migration pre-flight. v3 added
    /// `speakerNames`; v4 adds `extensionGrants` (both additive, never touching
    /// the four migrated data stores).
    #[test]
    fn rust_schema_version_maps_to_forty() {
        assert_eq!(RUST_SCHEMA_VERSION, 4);
        assert_eq!(expected_idb_version(RUST_SCHEMA_VERSION), 40);
        assert_eq!(SPEAKER_NAMES_STORE, "speakerNames");
        assert_eq!(EXTENSION_GRANTS_STORE, "extensionGrants");
    }

    /// The durable speaker-name map round-trips its camelCase `meetingId` key and
    /// keeps the rename map (Phase-F carry-forward).
    #[test]
    fn speaker_name_serde_roundtrip() {
        let mut names = std::collections::BTreeMap::new();
        names.insert("S1".to_owned(), "Alice".to_owned());
        names.insert("S2".to_owned(), "Bob".to_owned());
        let row = SpeakerName {
            meeting_id: 7,
            names,
        };
        let json = serde_json::to_string(&row).expect("serialize");
        assert!(json.contains("\"meetingId\":7"), "camelCase key: {json}");
        let back: SpeakerName = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(row, back);
        assert_eq!(back.names.get("S1").map(String::as_str), Some("Alice"));
    }

    #[test]
    fn snapshot_counts_sum_screenshot_bytes() {
        let snap = StorageSnapshot {
            meetings: vec![Meeting {
                id: 1,
                title: "m".into(),
                start_time: 0.0,
                end_time: None,
                duration: 0.0,
            }],
            transcript_chunks: vec![],
            notes: vec![],
            screenshots: vec![
                Screenshot {
                    id: 1,
                    meeting_id: 1,
                    timestamp: 0.0,
                    image: vec![1, 2, 3],
                    image_encoding: Some(ImageEncoding::Bytes),
                    width: 1,
                    height: 1,
                    analyzed: false,
                    analysis: String::new(),
                },
                Screenshot {
                    id: 2,
                    meeting_id: 1,
                    timestamp: 0.0,
                    image: vec![4, 5],
                    image_encoding: Some(ImageEncoding::Base64String),
                    width: 1,
                    height: 1,
                    analyzed: false,
                    analysis: String::new(),
                },
            ],
        };
        let c = snap.counts();
        assert_eq!(c.meetings, 1);
        assert_eq!(c.screenshots, 2);
        assert_eq!(c.screenshot_bytes, 5);
    }

    #[test]
    fn storage_event_backup_ready_is_tagged() {
        let ev = StorageEvent::BackupReady {
            object_url: "blob:null/abc".into(),
            filename: "silent-notetaker-backup-1700000000000.json".into(),
            size_bytes: 4096,
            counts: StorageCounts {
                meetings: 2,
                transcript_chunks: 4,
                notes: 4,
                screenshots: 3,
                screenshot_bytes: 1175,
            },
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"backup_ready\""), "tagged: {json}");
        let back: StorageEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn storage_event_completed_round_trips() {
        let counts = StorageCounts {
            meetings: 2,
            transcript_chunks: 4,
            notes: 4,
            screenshots: 3,
            screenshot_bytes: 1175,
        };
        let ev = StorageEvent::Completed {
            before: counts,
            after: counts,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"completed\""));
        let back: StorageEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    /// Guard the camelCase TS keys against ts-rs serde-compat drift. `end_time`
    /// in particular needs an explicit `#[ts(rename)]`; this fails loudly if the
    /// rename is ever dropped (the UI types against `endTime`, not `end_time`).
    #[test]
    fn meeting_ts_decl_uses_camel_case_keys() {
        use ts_rs::TS;
        let decl = <Meeting as TS>::decl();
        // Match the FIELD declarations (`<key>: <type>`), not the doc comments —
        // the rationale comment legitimately mentions `end_time`.
        assert!(
            decl.contains("endTime: number | null"),
            "TS field must be `endTime: number | null`: {decl}"
        );
        assert!(
            !decl.contains("end_time:"),
            "TS must NOT emit a snake_case `end_time:` field: {decl}"
        );
        assert!(
            decl.contains("startTime: number"),
            "TS field must be `startTime: number`: {decl}"
        );
    }

    /// `endTime: null` must round-trip as `null` (Dexie writes `null`, not an
    /// absent key, for an in-progress meeting). `skip_serializing_if` was
    /// deliberately NOT used so the re-stored backup matches Dexie byte-for-byte.
    #[test]
    fn meeting_serializes_none_end_time_as_null() {
        let m = Meeting {
            id: 7,
            title: "In progress".into(),
            start_time: 1_700_000_000_000.0,
            end_time: None,
            duration: 0.0,
        };
        let json = serde_json::to_string(&m).expect("serialize");
        assert!(
            json.contains("\"endTime\":null"),
            "None must serialize as endTime:null, got {json}"
        );
    }

    #[test]
    fn migration_status_round_trips() {
        for s in [
            MigrationStatus::Pending,
            MigrationStatus::AwaitingBackup,
            MigrationStatus::Migrating,
            MigrationStatus::Complete,
            MigrationStatus::AlreadyMigrated,
        ] {
            let json = serde_json::to_string(&s).expect("serialize");
            let back: MigrationStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(s, back);
        }
        assert_eq!(
            serde_json::to_string(&MigrationStatus::AwaitingBackup).expect("serialize"),
            "\"awaiting_backup\""
        );
    }
}
