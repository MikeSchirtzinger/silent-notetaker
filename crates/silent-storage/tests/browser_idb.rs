//! Browser tests for silent-storage (wasm32 target only).
//!
//! Run via `wasm-pack test --headless --chrome -- --test browser_idb`.
//!
//! # Two tiers
//!
//! 1. **Runs headless today (no live DB needed):** the pure-Rust backup +
//!    base64 logic and the snapshot→summary conversion, executed in a REAL
//!    browser context. These are real tests, not mocks: they prove the encoder
//!    and the JS-object construction behave identically on wasm32 as on native.
//!
//! 2. **NEEDS-BROWSER-TEST (live IndexedDB):** the full open → read → migrate
//!    round-trip needs a Dexie v2 `SilentNotetaker` database populated by the
//!    fixture page. `wasm-bindgen-test`'s headless harness does not create that
//!    fixture, so the round-trip is exercised at WIRING TIME against the
//!    fixture/app DB (the S4 spike already proved it end-to-end in-browser:
//!    `docs/research/spike-storage.md`, EMPTY readback diff across all three
//!    screenshot encodings). The placeholder below SKIPS LOUDLY rather than
//!    passing silently — a bare green here would be a lie.

#![cfg(target_arch = "wasm32")]
// `expect`/`unwrap` are idiomatic in tests: a failure is a test bug, surfaced in
// the browser console by wasm-bindgen-test. The PRD lint config allows this in
// tests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use silent_storage::backup::{Backup, base64_encode};
use silent_storage::{ImageEncoding, Meeting, Note, Screenshot, StorageSnapshot, TranscriptChunk};

// ---------------------------------------------------------------------------
// Tier 1 — real wasm tests that need no live database.
// ---------------------------------------------------------------------------

/// The base64 encoder must produce the RFC 4648 vectors on wasm32 too (float/int
/// behavior has differed across targets historically; this guards that).
#[wasm_bindgen_test]
fn base64_rfc4648_vectors_in_browser() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64_encode(&[0x89, 0x50, 0x4e, 0x47]), "iVBORw==");
}

fn sample_snapshot() -> StorageSnapshot {
    StorageSnapshot {
        meetings: vec![Meeting {
            id: 1,
            title: "Browser Meeting".into(),
            start_time: 1_700_000_000_000.0,
            end_time: None,
            duration: 0.0,
        }],
        transcript_chunks: vec![TranscriptChunk {
            id: 1,
            meeting_id: 1,
            timestamp: 0.0,
            text: "hello from wasm".into(),
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
        screenshots: vec![Screenshot {
            id: 1,
            meeting_id: 1,
            timestamp: 1.0,
            image: vec![0x89, 0x50, 0x4e, 0x47],
            image_encoding: Some(ImageEncoding::Blob),
            width: 640,
            height: 480,
            analyzed: false,
            analysis: String::new(),
        }],
    }
}

/// Build a backup in the browser and confirm it serializes and round-trips —
/// the export-backup payload the migration offers before any write.
#[wasm_bindgen_test]
fn backup_builds_and_round_trips_in_browser() {
    let snap = sample_snapshot();
    let backup = Backup::from_snapshot(&snap);
    assert_eq!(backup.counts.meetings, 1);
    assert_eq!(backup.counts.screenshots, 1);
    assert_eq!(backup.screenshots[0].image_base64, "iVBORw==");

    let bytes = backup.to_json_bytes().expect("serialize backup");
    let back: Backup = serde_json::from_slice(&bytes).expect("deserialize backup");
    assert_eq!(backup, back);
}

/// The snapshot→summary JS conversion must run in a real browser (it builds JS
/// objects via Reflect/Uint8Array). Confirms the counts the harness reads back.
#[wasm_bindgen_test]
fn snapshot_summary_builds_js_object() {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    let snap = sample_snapshot();
    let summary = silent_storage::summary::snapshot_to_summary(&snap).expect("summary builds");

    let get = |k: &str| Reflect::get(&summary, &JsValue::from_str(k)).expect("get");
    assert_eq!(get("meetingCount").as_f64(), Some(1.0));
    assert_eq!(get("screenshotCount").as_f64(), Some(1.0));
    // PNG magic is 4 bytes.
    assert_eq!(get("totalBlobBytes").as_f64(), Some(4.0));
}

// ---------------------------------------------------------------------------
// Tier 2 — the live-IndexedDB round-trip. SKIPS LOUDLY (see module docs).
// ---------------------------------------------------------------------------

/// The full open → read → migrate round-trip requires a Dexie v2 fixture DB this
/// headless harness does not create. Rather than pass silently (a lie), this
/// prints a LOUD bordered skip banner pointing at where the round-trip IS
/// proven. The wiring agent runs it against the fixture/app DB.
#[wasm_bindgen_test]
fn live_idb_migration_round_trip_needs_fixture_db() {
    web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(
        "\n\
         ╔══════════════════════════════════════════════════════════════════════╗\n\
         ║  LOUD SKIP — silent-storage live IndexedDB migration round-trip        ║\n\
         ║                                                                        ║\n\
         ║  open → read → migrate → verify needs a Dexie v2 'SilentNotetaker'      ║\n\
         ║  database populated by the fixture page. wasm-bindgen-test's headless   ║\n\
         ║  harness does not create it, so this is NOT asserted here.              ║\n\
         ║                                                                        ║\n\
         ║  PROVEN end-to-end in the S4 spike (EMPTY readback diff across base64   ║\n\
         ║  / Blob / Uint8Array): docs/research/spike-storage.md.                  ║\n\
         ║  Wired + re-run against the live app DB at integration time (Task H4).  ║\n\
         ╚══════════════════════════════════════════════════════════════════════╝\n",
    ));
    // Intentionally no assertion: this test documents an un-run path loudly.
    // A future wiring step replaces this body with the fixture-backed round-trip.
}
