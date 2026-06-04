//! Snapshot → JS summary object (wasm32 only) for the browser test harness and
//! the wiring-layer smoke check.
//!
//! Produces a plain JS object the harness can deep-equal against the fixture it
//! wrote: per-table arrays, screenshot metadata WITH the discovered encoding tag
//! (so the harness can assert each layout — base64 / Blob / bytes — was actually
//! exercised), and the raw screenshot bytes as `Uint8Array`s for byte-exact
//! comparison.

use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use silent_core::storage::{Meeting, Note, Screenshot, StorageSnapshot, TranscriptChunk};

use crate::error::{Result, StorageError};

fn set(obj: &Object, key: &str, val: &JsValue) -> Result<()> {
    Reflect::set(obj, &JsValue::from_str(key), val)
        .map(|_| ())
        .map_err(|e| StorageError::Js(format!("Reflect::set({key}): {e:?}")))
}

// u32 → f64 is lossless (u32 < 2^53). usize counts on wasm32 are 32-bit, so
// usize → f64 is lossless too.
#[allow(
    clippy::cast_precision_loss,
    reason = "u32/usize-on-wasm32 are within f64's exact-integer range"
)]
fn num(n: u64) -> JsValue {
    JsValue::from_f64(n as f64)
}

/// Convert a full snapshot to a JS summary object.
///
/// # Errors
///
/// Returns [`StorageError::Js`] if any `Reflect::set` fails.
pub fn snapshot_to_summary(snap: &StorageSnapshot) -> Result<JsValue> {
    let summary = Object::new();
    let counts = snap.counts();

    set(&summary, "meetingCount", &num(counts.meetings as u64))?;
    set(
        &summary,
        "chunkCount",
        &num(counts.transcript_chunks as u64),
    )?;
    set(&summary, "noteCount", &num(counts.notes as u64))?;
    set(&summary, "screenshotCount", &num(counts.screenshots as u64))?;
    set(
        &summary,
        "totalBlobBytes",
        &num(counts.screenshot_bytes as u64),
    )?;

    set(&summary, "meetings", &meetings_to_js(&snap.meetings)?)?;
    set(
        &summary,
        "transcriptChunks",
        &chunks_to_js(&snap.transcript_chunks)?,
    )?;
    set(&summary, "notes", &notes_to_js(&snap.notes)?)?;
    set(
        &summary,
        "screenshotMeta",
        &screenshot_meta_to_js(&snap.screenshots)?,
    )?;
    set(
        &summary,
        "screenshotBlobs",
        &screenshot_blobs_to_js(&snap.screenshots),
    )?;

    Ok(summary.into())
}

fn meetings_to_js(meetings: &[Meeting]) -> Result<JsValue> {
    let arr = Array::new();
    for m in meetings {
        let obj = Object::new();
        set(&obj, "id", &num(u64::from(m.id)))?;
        set(&obj, "title", &JsValue::from_str(&m.title))?;
        set(&obj, "startTime", &JsValue::from_f64(m.start_time))?;
        set(
            &obj,
            "endTime",
            &m.end_time.map_or(JsValue::NULL, JsValue::from_f64),
        )?;
        set(&obj, "duration", &JsValue::from_f64(m.duration))?;
        arr.push(&obj);
    }
    Ok(arr.into())
}

fn chunks_to_js(chunks: &[TranscriptChunk]) -> Result<JsValue> {
    let arr = Array::new();
    for c in chunks {
        let obj = Object::new();
        set(&obj, "id", &num(u64::from(c.id)))?;
        set(&obj, "meetingId", &num(u64::from(c.meeting_id)))?;
        set(&obj, "timestamp", &JsValue::from_f64(c.timestamp))?;
        set(&obj, "text", &JsValue::from_str(&c.text))?;
        set(&obj, "isFinal", &JsValue::from_bool(c.is_final))?;
        arr.push(&obj);
    }
    Ok(arr.into())
}

fn notes_to_js(notes: &[Note]) -> Result<JsValue> {
    let arr = Array::new();
    for n in notes {
        let obj = Object::new();
        set(&obj, "id", &num(u64::from(n.id)))?;
        set(&obj, "meetingId", &num(u64::from(n.meeting_id)))?;
        set(&obj, "category", &JsValue::from_str(&n.category))?;
        set(&obj, "text", &JsValue::from_str(&n.text))?;
        set(&obj, "timestamp", &JsValue::from_f64(n.timestamp))?;
        set(&obj, "triggerPhrase", &JsValue::from_str(&n.trigger_phrase))?;
        arr.push(&obj);
    }
    Ok(arr.into())
}

fn screenshot_meta_to_js(screenshots: &[Screenshot]) -> Result<JsValue> {
    let arr = Array::new();
    for s in screenshots {
        let obj = Object::new();
        set(&obj, "id", &num(u64::from(s.id)))?;
        set(&obj, "meetingId", &num(u64::from(s.meeting_id)))?;
        set(&obj, "timestamp", &JsValue::from_f64(s.timestamp))?;
        set(&obj, "width", &num(u64::from(s.width)))?;
        set(&obj, "height", &num(u64::from(s.height)))?;
        set(&obj, "analyzed", &JsValue::from_bool(s.analyzed))?;
        set(&obj, "analysis", &JsValue::from_str(&s.analysis))?;
        set(&obj, "blobByteLen", &num(s.image.len() as u64))?;
        let enc = s
            .image_encoding
            .map_or("empty", silent_core::storage::ImageEncoding::as_str);
        set(&obj, "encoding", &JsValue::from_str(enc))?;
        arr.push(&obj);
    }
    Ok(arr.into())
}

fn screenshot_blobs_to_js(screenshots: &[Screenshot]) -> JsValue {
    let arr = Array::new();
    for s in screenshots {
        arr.push(&Uint8Array::from(s.image.as_slice()));
    }
    arr.into()
}
