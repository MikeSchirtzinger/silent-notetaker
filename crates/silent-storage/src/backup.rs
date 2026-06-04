//! The export-backup serializer — pure Rust, native-testable.
//!
//! Before ANY migration write, `silent-storage` produces a self-contained JSON
//! backup of the entire `SilentNotetaker` database and emits a
//! [`silent_core::storage::StorageEvent::BackupReady`] so the UI can offer it as
//! a download (PRD Phase 4 exit criterion / Key Risk mitigation:
//! "export-backup before migrate").
//!
//! The backup is a single JSON document with all four tables. Screenshot binary
//! payloads are base64-encoded into the JSON so the file is portable and
//! self-contained — it can be re-imported without any external blob store. For a
//! screenshot already stored as a base64 data-URL string (the current app
//! layout), the bytes ARE the UTF-8 of that string, so the backup carries the
//! exact value `<img src>` reads.
//!
//! This module has no browser dependency: it turns a
//! [`silent_core::storage::StorageSnapshot`] into bytes, so it is exercised by
//! native unit tests (the byte-exactness that matters for "meetings survive the
//! upgrade" is provable without a browser).

use silent_core::storage::{ImageEncoding, Screenshot, StorageCounts, StorageSnapshot};

/// The on-disk backup envelope. Versioned so a future importer can branch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Backup {
    /// Backup format version (independent of the Dexie schema version).
    pub backup_format: u32,
    /// The Dexie user-visible schema version the source DB was at (`2`).
    pub source_dexie_version: u32,
    /// Row counts captured at backup time (a quick integrity check on import).
    pub counts: StorageCounts,
    /// All `meetings` rows.
    pub meetings: Vec<silent_core::storage::Meeting>,
    /// All `transcriptChunks` rows.
    pub transcript_chunks: Vec<silent_core::storage::TranscriptChunk>,
    /// All `notes` rows.
    pub notes: Vec<silent_core::storage::Note>,
    /// All `screenshots` rows, with binary payloads base64-encoded.
    pub screenshots: Vec<BackupScreenshot>,
}

/// A screenshot in the backup: the typed metadata plus the image payload as a
/// base64 string and a tag recording its original storage encoding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct BackupScreenshot {
    /// Auto-incrementing primary key.
    pub id: u32,
    /// Foreign key to `meetings.id`.
    pub meeting_id: u32,
    /// Capture time offset milliseconds.
    pub timestamp: f64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Whether Claude-bridge analysis ran.
    pub analyzed: bool,
    /// Claude-bridge analysis text.
    pub analysis: String,
    /// Original storage encoding tag (`base64` / `blob` / `bytes` / `empty`), so
    /// an importer knows how the bytes were stored before normalization.
    pub original_encoding: String,
    /// The raw image payload bytes, standard-base64-encoded.
    pub image_base64: String,
}

/// The format version this build writes.
pub const BACKUP_FORMAT_VERSION: u32 = 1;

/// The Dexie schema version of the source DB recorded in backups.
pub const SOURCE_DEXIE_VERSION: u32 = silent_core::storage::DEXIE_SCHEMA_VERSION;

impl Backup {
    /// Build a backup envelope from a full database snapshot.
    #[must_use]
    pub fn from_snapshot(snap: &StorageSnapshot) -> Self {
        Backup {
            backup_format: BACKUP_FORMAT_VERSION,
            source_dexie_version: SOURCE_DEXIE_VERSION,
            counts: snap.counts(),
            meetings: snap.meetings.clone(),
            transcript_chunks: snap.transcript_chunks.clone(),
            notes: snap.notes.clone(),
            screenshots: snap.screenshots.iter().map(backup_screenshot).collect(),
        }
    }

    /// Serialize the backup to pretty JSON bytes (the download payload).
    ///
    /// # Errors
    ///
    /// Returns the `serde_json` error message if serialization fails (it should
    /// not, for these plain types — but the migration must not panic).
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|e| e.to_string())
    }

    /// A suggested download filename keyed on a timestamp (the UI passes
    /// `Date.now()`), for example `silent-notetaker-backup-1700000000000.json`.
    #[must_use]
    pub fn filename(timestamp_ms: f64) -> String {
        // The timestamp is a whole-millisecond epoch value; render without a
        // fractional part. `f64::trunc` keeps the integer portion; epoch ms is
        // well within f64's exact-integer range (< 2^53) for any real date.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "epoch-ms is a non-negative whole number well within u64/f64 \
                      exact-integer range for any realistic date"
        )]
        let ts = timestamp_ms.trunc() as u64;
        format!("silent-notetaker-backup-{ts}.json")
    }
}

fn backup_screenshot(s: &Screenshot) -> BackupScreenshot {
    BackupScreenshot {
        id: s.id,
        meeting_id: s.meeting_id,
        timestamp: s.timestamp,
        width: s.width,
        height: s.height,
        analyzed: s.analyzed,
        analysis: s.analysis.clone(),
        original_encoding: s
            .image_encoding
            .unwrap_or(ImageEncoding::Empty)
            .as_str()
            .to_owned(),
        image_base64: base64_encode(&s.image),
    }
}

// ---------------------------------------------------------------------------
// Standard base64 (RFC 4648, `+`/`/` alphabet, `=` padding), implemented inline
// to avoid a dependency. Encoding only — the migration writes backups; it never
// decodes them (re-import is a future, separate concern). Tested against the
// RFC 4648 §10 vectors below.
// ---------------------------------------------------------------------------

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard-base64-encode a byte slice (RFC 4648, with `=` padding).
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        // Pack up to three bytes into a 24-bit group.
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |&b| u32::from(b));
        let b2 = chunk.get(2).map_or(0, |&b| u32::from(b));
        let group = (b0 << 16) | (b1 << 8) | b2;

        // Indices are masked to 0..=63, always valid into B64_ALPHABET.
        out.push(B64_ALPHABET[((group >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((group >> 12) & 0x3f) as usize] as char);
        match chunk.len() {
            1 => {
                out.push('=');
                out.push('=');
            }
            2 => {
                out.push(B64_ALPHABET[((group >> 6) & 0x3f) as usize] as char);
                out.push('=');
            }
            _ => {
                out.push(B64_ALPHABET[((group >> 6) & 0x3f) as usize] as char);
                out.push(B64_ALPHABET[(group & 0x3f) as usize] as char);
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism (PRD lint config)"
)]
mod tests {
    use super::*;
    use silent_core::storage::{Meeting, Note, TranscriptChunk};

    /// RFC 4648 §10 test vectors — the canonical proof the encoder is correct.
    #[test]
    fn base64_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_binary_bytes() {
        // PNG magic + a couple of high bytes (exercises the full alphabet incl.
        // `+`/`/` regions and padding).
        assert_eq!(base64_encode(&[0x89, 0x50, 0x4e, 0x47]), "iVBORw==");
        assert_eq!(base64_encode(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(base64_encode(&[0xfb, 0xff, 0xbf]), "+/+/");
    }

    fn sample_snapshot() -> StorageSnapshot {
        StorageSnapshot {
            meetings: vec![Meeting {
                id: 1,
                title: "Q1".into(),
                start_time: 1_700_000_000_000.0,
                end_time: None,
                duration: 0.0,
            }],
            transcript_chunks: vec![TranscriptChunk {
                id: 1,
                meeting_id: 1,
                timestamp: 0.0,
                text: "hello".into(),
                is_final: true,
            }],
            notes: vec![Note {
                id: 1,
                meeting_id: 1,
                category: "decisions".into(),
                text: "ship it".into(),
                timestamp: 0.0,
                trigger_phrase: "we decided".into(),
            }],
            screenshots: vec![
                // Current-app layout: image bytes ARE the data-URL string bytes.
                Screenshot {
                    id: 1,
                    meeting_id: 1,
                    timestamp: 1.0,
                    image: b"data:image/jpeg;base64,/9j/".to_vec(),
                    image_encoding: Some(ImageEncoding::Base64String),
                    width: 1280,
                    height: 720,
                    analyzed: false,
                    analysis: String::new(),
                },
                // Migration-critical layout: real binary bytes.
                Screenshot {
                    id: 2,
                    meeting_id: 1,
                    timestamp: 2.0,
                    image: vec![0x89, 0x50, 0x4e, 0x47],
                    image_encoding: Some(ImageEncoding::Blob),
                    width: 640,
                    height: 480,
                    analyzed: true,
                    analysis: "a chart".into(),
                },
            ],
        }
    }

    #[test]
    fn backup_preserves_counts_and_screenshots() {
        let snap = sample_snapshot();
        let backup = Backup::from_snapshot(&snap);

        assert_eq!(backup.backup_format, BACKUP_FORMAT_VERSION);
        assert_eq!(backup.source_dexie_version, 2);
        assert_eq!(backup.counts.meetings, 1);
        assert_eq!(backup.counts.screenshots, 2);
        // base64 payload (27 bytes) + PNG magic (4 bytes) = 31 image bytes total.
        assert_eq!(backup.counts.screenshot_bytes, 27 + 4);

        // The base64-string screenshot's payload round-trips its data-URL bytes.
        let s0 = &backup.screenshots[0];
        assert_eq!(s0.original_encoding, "base64");
        assert_eq!(
            base64_encode(b"data:image/jpeg;base64,/9j/"),
            s0.image_base64
        );

        let s1 = &backup.screenshots[1];
        assert_eq!(s1.original_encoding, "blob");
        assert_eq!(s1.image_base64, "iVBORw==");
        assert_eq!(s1.analysis, "a chart");
    }

    #[test]
    fn backup_json_round_trips() {
        let snap = sample_snapshot();
        let backup = Backup::from_snapshot(&snap);
        let bytes = backup.to_json_bytes().expect("serialize backup");
        assert!(!bytes.is_empty());
        let back: Backup = serde_json::from_slice(&bytes).expect("deserialize backup");
        assert_eq!(backup, back);
    }

    #[test]
    fn filename_renders_integer_timestamp() {
        assert_eq!(
            Backup::filename(1_700_000_000_000.0),
            "silent-notetaker-backup-1700000000000.json"
        );
    }
}
