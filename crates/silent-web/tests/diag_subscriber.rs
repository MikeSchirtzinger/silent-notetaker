//! Native end-to-end test for the crash-diagnostics `tracing` subscriber
//! ([`silent_web::diag::DiagLayer`], Appendix A row 34).
//!
//! This drives the FULL path the wasm subscriber uses — real `tracing` events on
//! the `silent.diag` target, dispatched through `DiagLayer`, written through a
//! JSON-encoding sink — and asserts the stored `notetakerDiag` bytes are
//! byte-identical to the JS golden fixture captured in
//! `silent-core/goldens/gen/diag_ref.mjs`. The only difference from the browser
//! is the storage backend (`MemRawStore` here, `localStorage` in wasm) and the
//! field sources (the test supplies the host values the wasm reader would read
//! from `performance.memory` / the DOM). The translation + serialization are the
//! same code.
//!
//! It also exercises the PerfMonitor (row 35) path: a `stats` event on the same
//! target updates `DiagLayer::take_stats`, proving the crash trail and the perf
//! snapshot are unified on one subscriber.
//!
//! Native-only (the `DiagLayer` translation logic is browser-free by design).
#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism (PRD lint config)"
)]

use std::path::PathBuf;

use serde::Deserialize;
use silent_core::diag::schema;
use silent_web::diag::{DiagLayer, JsonSink, MemRawStore};
use tracing_subscriber::layer::SubscriberExt;

// ---------------------------------------------------------------------------
// Golden fixture loading (the silent-core fixture, reached by relative path).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Fixture {
    rows: Vec<RowCase>,
}

#[derive(Debug, Deserialize)]
struct RowCase {
    input: RowInput,
    #[serde(rename = "rowJson")]
    row_json: String,
    #[serde(rename = "arrayJson")]
    array_json: String,
}

#[derive(Debug, Deserialize)]
struct RowInput {
    iso: String,
    #[serde(rename = "elapsedSec")]
    elapsed_sec: u64,
    mem: Option<MemInput>,
    items: u64,
    words: u64,
    #[serde(rename = "inputTokens")]
    input_tokens: u64,
    #[serde(rename = "lastStepMs")]
    last_step_ms: f64,
    #[serde(rename = "writeAbs")]
    write_abs: Option<u64>,
    #[serde(rename = "genStepsTotal")]
    gen_steps_total: u64,
}

#[derive(Debug, Deserialize)]
struct MemInput {
    used: f64,
    total: f64,
    limit: f64,
}

fn load_fixture() -> Fixture {
    // crates/silent-web/tests -> crates/silent-core/goldens/diag/diag.json
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../silent-core/goldens/diag/diag.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    serde_json::from_str(&raw).expect("parse diag.json")
}

// ---------------------------------------------------------------------------
// Event emitters — exactly what the wasm host would emit per the schema.
// ---------------------------------------------------------------------------

fn emit_loop_iter(input_tokens: u64) {
    tracing::info!(
        target: "silent.diag",
        diag_kind = schema::KIND_LOOP_ITER,
        input_tokens = input_tokens,
    );
}

fn emit_put(now_ms: f64, n_tokens: u64) {
    tracing::info!(
        target: "silent.diag",
        diag_kind = schema::KIND_PUT,
        now_ms = now_ms,
        n_tokens = n_tokens,
    );
}

fn emit_sample(c: &RowInput) {
    // The host-supplied sample fields. Heap is split into present-flag + bytes
    // because a tracing field cannot be None.
    let (present, used, total, limit) = match &c.mem {
        Some(m) => (true, m.used, m.total, m.limit),
        None => (false, 0.0, 0.0, 0.0),
    };
    // write_abs uses -1 as the "no ring" sentinel.
    let write_abs: i64 = c.write_abs.map_or(-1, |w| i64::try_from(w).unwrap());
    tracing::info!(
        target: "silent.diag",
        diag_kind = schema::KIND_SAMPLE,
        iso = c.iso.as_str(),
        elapsed_sec = c.elapsed_sec,
        heap_present = present,
        heap_used_bytes = used,
        heap_total_bytes = total,
        heap_limit_bytes = limit,
        items = c.items,
        words = c.words,
        write_abs = write_abs,
    );
}

fn emit_device_lost_sample(msg: &str, c: &RowInput) {
    // A device-lost event carries the message AND the host-clocked sample fields
    // (the JS onDeviceLost took an immediate sample). One event does both.
    let (present, used, total, limit) = match &c.mem {
        Some(m) => (true, m.used, m.total, m.limit),
        None => (false, 0.0, 0.0, 0.0),
    };
    let write_abs: i64 = c.write_abs.map_or(-1, |w| i64::try_from(w).unwrap());
    tracing::warn!(
        target: "silent.diag",
        diag_kind = schema::KIND_DEVICE_LOST,
        message = msg,
        iso = c.iso.as_str(),
        elapsed_sec = c.elapsed_sec,
        heap_present = present,
        heap_used_bytes = used,
        heap_total_bytes = total,
        heap_limit_bytes = limit,
        items = c.items,
        words = c.words,
        write_abs = write_abs,
    );
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Build a subscriber around a `DiagLayer` and run `body` with it as the default
/// dispatcher, returning a handle so the test can read back the stored rows.
/// `DiagLayer` is `Clone` and shares its state, so we install one clone and keep
/// a handle.
fn with_layer<F: FnOnce(&DiagLayer<JsonSink<MemRawStore>>)>(
    body: F,
) -> DiagLayer<JsonSink<MemRawStore>> {
    let layer = DiagLayer::new(JsonSink::new(MemRawStore::new()));
    let handle = layer.handle();
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || body(&handle));
    handle
}

#[test]
fn subscriber_writes_golden_bytes_for_each_tick() {
    let f = load_fixture();
    let layer = with_layer(|layer| {
        // Drive the same sequence the JS sampler did, through tracing events.
        // Tick 0: baseline (no loop activity yet).
        emit_sample(&f.rows[0].input);
        assert_eq!(
            serde_json::to_string(&layer.rows()).unwrap(),
            f.rows[0].array_json,
            "ring after tick 0"
        );

        // Tick 1: one generate context, puts ending on the recorded gap.
        let c1 = &f.rows[1].input;
        emit_loop_iter(c1.input_tokens);
        emit_put(0.0, 1); // primes last_put_at
        for k in 1..(c1.gen_steps_total - 1) {
            emit_put(f64::from(u32::try_from(k).unwrap()), 1);
        }
        let prev = f64::from(u32::try_from(c1.gen_steps_total - 2).unwrap());
        emit_put(prev + c1.last_step_ms, 1);
        emit_sample(c1);
        assert_eq!(
            serde_json::to_string(&layer.rows()).unwrap(),
            f.rows[1].array_json,
            "ring after tick 1"
        );
    });

    // Final stored value equals the row-1 cumulative array.
    let rows = layer.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(serde_json::to_string(&rows[0]).unwrap(), f.rows[0].row_json);
    assert_eq!(serde_json::to_string(&rows[1]).unwrap(), f.rows[1].row_json);
}

#[test]
fn device_lost_event_records_message_and_samples() {
    let f = load_fixture();
    // Use the device-lost fixture row (row 4) as the host sample payload.
    let c = &f.rows[4].input;
    let layer = with_layer(|_layer| {
        emit_device_lost_sample("Device lost: out of memory while allocating buffer", c);
    });
    let rows = layer.rows();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.device_lost,
        "Device lost: out of memory while allocating buffer"
    );
    // The device-lost row's heap + elapsed match the supplied sample input.
    assert_eq!(row.elapsed_sec, c.elapsed_sec);
    assert_eq!(row.heap_used_mb.unwrap().to_string(), "2008");
    // The summary line surfaces the DEVICE-LOST tag.
    let line = silent_core::diag::prior_trail::summary_line(row);
    assert!(
        line.contains("DEVICE-LOST:Device lost: out of memory"),
        "{line}"
    );
}

#[test]
fn reset_clears_prior_trail() {
    let layer = with_layer(|layer| {
        // Two samples, then reset, then one more.
        emit_sample(&load_fixture().rows[0].input);
        emit_sample(&load_fixture().rows[0].input);
        assert_eq!(layer.rows().len(), 2);
        layer.reset();
        assert_eq!(layer.rows().len(), 0, "reset clears the trail");
        emit_sample(&load_fixture().rows[0].input);
    });
    assert_eq!(layer.rows().len(), 1, "one row after reset + sample");
}

#[test]
fn stats_event_feeds_perfmonitor() {
    // PerfMonitor (row 35): a `stats` event on the same target updates
    // take_stats with the EngineStats fields.
    let layer = with_layer(|layer| {
        tracing::info!(
            target: "silent.diag",
            diag_kind = schema::KIND_STATS,
            load_ms = 1200u64,
            chunks = 48u64,
            avg_chunk_ms = 70u64,
            last_chunk_ms = 65u64,
            audio_secs = 12.5f64,
            rtf = 0.28f64,
            ttft_ms = 650u64,
            pending_samples = 320u64,
        );
        let stats = layer.take_stats().expect("stats recorded");
        assert_eq!(stats.load_ms, 1200);
        assert_eq!(stats.chunks, 48);
        assert_eq!(stats.avg_chunk_ms, 70);
        assert_eq!(stats.last_chunk_ms, 65);
        assert!((stats.audio_secs - 12.5).abs() < 1e-6);
        assert!((stats.rtf - 0.28).abs() < 1e-6);
        assert_eq!(stats.ttft_ms, 650);
        assert_eq!(stats.pending_samples, 320);
        // taken once -> now empty.
        assert!(layer.take_stats().is_none());
    });
    // A stats event does NOT write a diag row (it is telemetry, not a sample).
    assert_eq!(layer.rows().len(), 0);
}

#[test]
fn unrelated_target_is_ignored() {
    let layer = with_layer(|layer| {
        tracing::info!(target: "some.other.target", diag_kind = "sample", iso = "x");
        assert_eq!(
            layer.rows().len(),
            0,
            "non-diag target must not touch the ring"
        );
    });
    let _ = layer;
}

/// `DiagLayer` is `Send + Sync` (it is a `tracing` Layer behind a `Mutex`), so
/// installing it as a global subscriber is sound.
#[test]
fn layer_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DiagLayer<JsonSink<MemRawStore>>>();
}
