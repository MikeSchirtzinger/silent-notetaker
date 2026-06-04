//! The crash-diagnostics `tracing` subscriber (PRD Phase 5, Appendix A row 34).
//!
//! Crash diagnostics are "formalized on `tracing` with a wasm subscriber": the
//! engine emits the loop signals and the ~3 s sample as `tracing` events on the
//! [`silent_core::diag::schema`] contract, and [`DiagLayer`] — a
//! [`tracing_subscriber::Layer`] — folds them into a [`silent_core::Diag`]
//! sampler, writing the bounded ring through a [`silent_core::StorageSink`].
//!
//! # Where the byte-identity lives
//!
//! The localStorage row format (the `notetakerDiag` value) is owned by
//! `silent-core`'s [`silent_core::DiagRow`] / [`silent_core::OneDecimal`] and is
//! proven byte-identical to the JS in `silent-core/tests/diag_golden.rs`. This
//! layer only *drives* that policy and serializes the rows with `serde_json`
//! (the JSON↔storage encoding the JS `JSON.stringify(rows)` did). So the trail
//! this subscriber writes is interchangeable with a JS-written one.
//!
//! # Native-testable, wasm-real
//!
//! [`DiagLayer`] is generic over the [`silent_core::StorageSink`] and depends
//! only on browser-free crates (`tracing`, `tracing-subscriber`, `serde_json`),
//! so its translation logic is unit-tested on the NATIVE target against an
//! in-memory sink (see `tests/diag_subscriber.rs`). Only [`LocalStorageSink`]
//! (and the small `performance.memory` / `performance.now` readers) are
//! `wasm32`-only — that is the genuinely browser-bound part, kept thin.
//!
//! # PerfMonitor (row 35) on the same target
//!
//! A `stats` event (the [`silent_core::EngineStats`] fields) rides the SAME
//! `silent.diag` target, so one subscriber sees both the crash trail and the
//! perf snapshot. [`DiagLayer`] exposes the latest stats via
//! [`DiagLayer::take_stats`] so a PerfMonitor surface can read them without a
//! second subscriber; the crash ring and the PerfMonitor are unified on
//! `tracing` exactly as the PRD requires.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use silent_core::diag::{HeapBytes, SampleInput, schema};
use silent_core::{Diag, DiagRow, EngineStats, StorageSink};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// A JSON-encoding [`StorageSink`] wrapper.
///
/// [`silent_core::StorageSink`] is typed (it carries `&[DiagRow]`, keeping
/// `silent-core` free of a JSON codec). The real storage is a string store
/// (`localStorage`), so this trait bridges the two: it owns the
/// `serde_json::to_string` / `from_str` encoding — the exact JS
/// `JSON.stringify(rows)` / `JSON.parse` — and delegates the raw string
/// read/write to a [`RawStore`]. [`LocalStorageSink`] and the test
/// [`MemRawStore`] both implement [`RawStore`].
pub trait RawStore {
    /// Read the raw stored string for `key`, or `None` if absent.
    fn get_raw(&self, key: &str) -> Option<String>;
    /// Write the raw string for `key`. Failures (quota) are swallowed.
    fn set_raw(&mut self, key: &str, value: &str);
}

/// A [`StorageSink`] that JSON-encodes [`DiagRow`]s over a [`RawStore`].
///
/// `read` maps a `JSON.parse` failure to an empty trail (the JS
/// `try { JSON.parse } catch { [] }`); `write` serializes to the exact
/// `JSON.stringify(rows)` bytes, falling back to `"[]"` on the (impossible)
/// serialization error so storage never holds garbage and the path needs no
/// `unwrap`.
pub struct JsonSink<R: RawStore> {
    raw: R,
}

impl<R: RawStore> JsonSink<R> {
    /// Wrap a raw string store.
    pub fn new(raw: R) -> Self {
        Self { raw }
    }

    /// Borrow the underlying raw store (tests read it back).
    pub fn raw(&self) -> &R {
        &self.raw
    }
}

impl<R: RawStore> StorageSink for JsonSink<R> {
    fn read(&self, key: &str) -> Vec<DiagRow> {
        match self.raw.get_raw(key) {
            Some(s) => serde_json::from_str(&s).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn write(&mut self, key: &str, rows: &[DiagRow]) {
        let json = serde_json::to_string(rows).unwrap_or_else(|_| "[]".to_owned());
        self.raw.set_raw(key, &json);
    }
}

/// An in-memory [`RawStore`] for native tests (and any non-browser host).
#[derive(Debug, Default)]
pub struct MemRawStore {
    map: std::collections::BTreeMap<String, String>,
}

impl MemRawStore {
    /// A fresh empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// The raw stored value for `key` (the exact bytes a `dumpDiag()` parses).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }
}

impl RawStore for MemRawStore {
    fn get_raw(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }
    fn set_raw(&mut self, key: &str, value: &str) {
        self.map.insert(key.to_owned(), value.to_owned());
    }
}

// ===========================================================================
// DiagLayer — the tracing Layer translating diag events into Diag operations.
// ===========================================================================

/// Mutable state behind the layer's `Mutex` (a `tracing` `Layer` is `Sync`, so
/// the sampler + sink + last-stats live behind one lock).
struct Inner<S: StorageSink> {
    diag: Diag,
    sink: S,
    last_stats: Option<EngineStats>,
}

/// The crash-diagnostics [`Layer`].
///
/// Add it to a `tracing` subscriber (`registry().with(DiagLayer::new(sink))`).
/// It filters to the [`schema::TARGET`] target and dispatches each event by its
/// [`schema::KIND`] field into the matching [`Diag`] hook or
/// [`Diag::sample`]. Events on other targets are ignored.
///
/// The state lives behind `Arc<Mutex<…>>`, so [`handle`](DiagLayer::handle)
/// returns a cheap clone sharing the SAME sampler + sink. Install one clone in
/// the subscriber and keep the handle to call [`reset`](DiagLayer::reset) /
/// [`take_stats`](DiagLayer::take_stats) / [`rows`](DiagLayer::rows) (the wasm
/// glue holds the handle; the registry owns the installed clone).
pub struct DiagLayer<S: StorageSink> {
    inner: Arc<Mutex<Inner<S>>>,
}

impl<S: StorageSink> Clone for DiagLayer<S> {
    /// A cheap clone sharing the same underlying sampler + sink (refcount bump).
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S: StorageSink> DiagLayer<S> {
    /// Build the layer over a storage sink. The [`Diag`] starts fresh; a
    /// `start` event (or the host calling [`reset`](DiagLayer::reset)) clears the
    /// prior trail.
    pub fn new(sink: S) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                diag: Diag::new(),
                sink,
                last_stats: None,
            })),
        }
    }

    /// A handle sharing the same state — install one clone in the subscriber and
    /// keep another to drive `reset` / read `rows` / poll `take_stats`.
    #[must_use]
    pub fn handle(&self) -> Self {
        self.clone()
    }

    /// Begin a fresh trail (mirrors `Diag.start()`): zero the counters and clear
    /// the stored trail. The host calls this at Voxtral-session start; the timer
    /// + baseline sample are then driven by `sample` events.
    pub fn reset(&self) {
        if let Ok(mut g) = self.inner.lock() {
            let Inner { diag, sink, .. } = &mut *g;
            diag.start(sink);
        }
    }

    /// Take the most recent [`EngineStats`] snapshot seen on a `stats` event
    /// (row 35), leaving `None` behind. A PerfMonitor surface polls this.
    #[must_use]
    pub fn take_stats(&self) -> Option<EngineStats> {
        self.inner.lock().ok().and_then(|mut g| g.last_stats.take())
    }

    /// The current trail rows (test/diagnostic accessor — reads through the
    /// sink). Returns empty on a poisoned lock rather than panicking.
    #[must_use]
    pub fn rows(&self) -> Vec<DiagRow> {
        match self.inner.lock() {
            Ok(g) => g.sink.read(silent_core::DIAG_KEY),
            Err(_) => Vec::new(),
        }
    }
}

impl<Sub, Snk> Layer<Sub> for DiagLayer<Snk>
where
    Sub: Subscriber,
    Snk: StorageSink + 'static,
{
    /// Translate one `tracing` event into a [`Diag`] operation.
    ///
    /// Only events on [`schema::TARGET`] are handled; others are ignored (the
    /// crash ring never sees unrelated spans). The event's [`schema::KIND`]
    /// field selects the operation; a [`DiagVisit`] collects the fields the
    /// operation needs. Lock poisoning is treated as "drop this event" rather
    /// than a panic — diagnostics must never crash the app they observe.
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, Sub>) {
        if event.metadata().target() != schema::TARGET {
            return;
        }
        let mut v = DiagVisit::default();
        event.record(&mut v);
        let Some(kind) = v.kind.as_deref() else {
            return;
        };
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        let Inner {
            diag,
            sink,
            last_stats,
        } = &mut *g;
        match kind {
            schema::KIND_LOOP_ITER => diag.on_loop_iter(v.input_tokens.unwrap_or(0)),
            schema::KIND_RECYCLE => diag.on_recycle(),
            schema::KIND_PUT => {
                diag.on_put(v.now_ms.unwrap_or(0.0), v.n_tokens.unwrap_or(0));
            }
            schema::KIND_DEVICE_LOST => {
                diag.on_device_lost(v.message.as_deref().unwrap_or(""));
                // The JS `onDeviceLost` took an immediate out-of-band sample.
                // Reproduce it when the event carried the host-clocked sample
                // fields (a device-lost event SHOULD carry them); if it did not,
                // the next ~3 s `sample` tick records the message anyway.
                if let Some(input) = v.to_sample_input() {
                    let _ = diag.sample(&input, sink);
                }
            }
            schema::KIND_SAMPLE => {
                if let Some(input) = v.to_sample_input() {
                    let _ = diag.sample(&input, sink);
                }
            }
            schema::KIND_STATS => {
                if let Some(stats) = v.to_engine_stats() {
                    *last_stats = Some(stats);
                }
            }
            _ => {}
        }
    }
}

/// A [`Visit`] that pulls the diag fields off a `tracing` event. Every field is
/// optional; the dispatch in `on_event` supplies defaults where the JS did.
#[derive(Default)]
struct DiagVisit {
    kind: Option<String>,
    // loop_iter / put / device_lost
    input_tokens: Option<u64>,
    now_ms: Option<f64>,
    n_tokens: Option<u64>,
    message: Option<String>,
    // sample (SampleInput)
    iso: Option<String>,
    elapsed_sec: Option<u64>,
    heap_present: Option<bool>,
    heap_used: Option<f64>,
    heap_total: Option<f64>,
    heap_limit: Option<f64>,
    items: Option<u64>,
    words: Option<u64>,
    write_abs: Option<i64>,
    // stats (EngineStats)
    load_ms: Option<u64>,
    chunks: Option<u64>,
    avg_chunk_ms: Option<u64>,
    last_chunk_ms: Option<u64>,
    audio_secs: Option<f64>,
    rtf: Option<f64>,
    ttft_ms: Option<u64>,
    pending_samples: Option<u64>,
}

impl DiagVisit {
    /// Assemble a [`SampleInput`] from the recorded sample fields, or `None` if
    /// the mandatory `iso` is missing (an ill-formed sample event is dropped).
    fn to_sample_input(&self) -> Option<SampleInput> {
        let iso = self.iso.clone()?;
        let heap = if self.heap_present.unwrap_or(false) {
            Some(HeapBytes {
                used: self.heap_used.unwrap_or(0.0),
                total: self.heap_total.unwrap_or(0.0),
                limit: self.heap_limit.unwrap_or(0.0),
            })
        } else {
            None
        };
        // `-1` is the agreed "no ring" sentinel -> None (the JS `null` writeAbs).
        let write_abs = match self.write_abs {
            Some(w) if w >= 0 => u64::try_from(w).ok(),
            _ => None,
        };
        Some(SampleInput {
            iso,
            elapsed_sec: self.elapsed_sec.unwrap_or(0),
            heap,
            items: self.items.unwrap_or(0),
            words: self.words.unwrap_or(0),
            write_abs,
        })
    }

    /// Assemble an [`EngineStats`] from the recorded stats fields. Returns `None`
    /// only if nothing stats-shaped was present (no `load_ms` and no `chunks`),
    /// so a non-stats event mislabeled `stats` is ignored rather than recording
    /// zeros.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "EngineStats fields are u32; the tracing wire carries them as \
                  u64/f64 (the only numeric kinds tracing records). The values \
                  are small latencies/counts well within u32, and saturating to \
                  u32::MAX on the impossible overflow is safe for a telemetry \
                  snapshot."
    )]
    fn to_engine_stats(&self) -> Option<EngineStats> {
        if self.load_ms.is_none() && self.chunks.is_none() {
            return None;
        }
        let u = |x: Option<u64>| u32::try_from(x.unwrap_or(0)).unwrap_or(u32::MAX);
        Some(EngineStats {
            load_ms: u(self.load_ms),
            chunks: u(self.chunks),
            avg_chunk_ms: u(self.avg_chunk_ms),
            last_chunk_ms: u(self.last_chunk_ms),
            audio_secs: self.audio_secs.unwrap_or(0.0) as f32,
            rtf: self.rtf.unwrap_or(0.0) as f32,
            ttft_ms: u(self.ttft_ms),
            pending_samples: u(self.pending_samples),
        })
    }
}

impl Visit for DiagVisit {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            schema::KIND => self.kind = Some(value.to_owned()),
            schema::F_MESSAGE => self.message = Some(value.to_owned()),
            schema::F_ISO => self.iso = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            schema::F_INPUT_TOKENS => self.input_tokens = Some(value),
            schema::F_N_TOKENS => self.n_tokens = Some(value),
            schema::F_ELAPSED_SEC => self.elapsed_sec = Some(value),
            schema::F_ITEMS => self.items = Some(value),
            schema::F_WORDS => self.words = Some(value),
            "load_ms" => self.load_ms = Some(value),
            "chunks" => self.chunks = Some(value),
            "avg_chunk_ms" => self.avg_chunk_ms = Some(value),
            "last_chunk_ms" => self.last_chunk_ms = Some(value),
            "ttft_ms" => self.ttft_ms = Some(value),
            "pending_samples" => self.pending_samples = Some(value),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        match field.name() {
            schema::F_WRITE_ABS => self.write_abs = Some(value),
            // tolerate counters recorded as i64
            schema::F_INPUT_TOKENS => self.input_tokens = u64::try_from(value).ok(),
            schema::F_N_TOKENS => self.n_tokens = u64::try_from(value).ok(),
            schema::F_ELAPSED_SEC => self.elapsed_sec = u64::try_from(value).ok(),
            schema::F_ITEMS => self.items = u64::try_from(value).ok(),
            schema::F_WORDS => self.words = u64::try_from(value).ok(),
            _ => {}
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        match field.name() {
            schema::F_NOW_MS => self.now_ms = Some(value),
            schema::F_HEAP_USED => self.heap_used = Some(value),
            schema::F_HEAP_TOTAL => self.heap_total = Some(value),
            schema::F_HEAP_LIMIT => self.heap_limit = Some(value),
            "audio_secs" => self.audio_secs = Some(value),
            "rtf" => self.rtf = Some(value),
            _ => {}
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == schema::F_HEAP_PRESENT {
            self.heap_present = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {
        // Diag fields are recorded with typed `record_*` calls; debug-only
        // fields are not part of the schema and are ignored.
    }
}

// ===========================================================================
// JSON serialization note: the rows are serialized inside `JsonSink::write`
// above. `Serialize` is in scope only to make that explicit at the use site.
// ===========================================================================
const _: fn() = || {
    fn assert_serialize<T: Serialize>() {}
    assert_serialize::<DiagRow>();
};

// ===========================================================================
// Browser-bound pieces (wasm32 only): the real localStorage RawStore and the
// `performance.memory` / `performance.now()` readers the host uses to build
// `sample`/`put` tracing events. These are the genuinely browser-coupled parts;
// everything above is native-testable.
// ===========================================================================
#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{JsonSink, RawStore};
    use silent_core::diag::HeapBytes;

    /// A [`RawStore`] backed by `window.localStorage` — the real
    /// `notetakerDiag` persistence. Read/write failures (no window, no storage,
    /// quota, a security exception) are swallowed, matching the JS
    /// `try { … } catch { /* drop */ }`, so diagnostics never throw into the app.
    pub struct LocalStorageSink;

    impl LocalStorageSink {
        /// The `JsonSink<LocalStorageSink>` the [`super::DiagLayer`] takes — a
        /// localStorage-backed, JSON-encoding [`silent_core::StorageSink`].
        #[must_use]
        pub fn json() -> JsonSink<LocalStorageSink> {
            JsonSink::new(LocalStorageSink)
        }

        fn storage() -> Option<web_sys::Storage> {
            // `local_storage()` returns Result<Option<Storage>, JsValue>; flatten
            // both failure shapes to None.
            web_sys::window()?.local_storage().ok().flatten()
        }
    }

    impl RawStore for LocalStorageSink {
        fn get_raw(&self, key: &str) -> Option<String> {
            Self::storage()?.get_item(key).ok().flatten()
        }

        fn set_raw(&mut self, key: &str, value: &str) {
            if let Some(s) = Self::storage() {
                // Ignore quota / security errors (drop silently, as the JS did).
                let _ = s.set_item(key, value);
            }
        }
    }

    /// Read `performance.memory` if the engine exposes it (Chromium only). The
    /// returned bytes feed the `sample` event's `heap_*` fields; `None` on
    /// Firefox/Safari drives the null-heap rows — exactly the JS
    /// `performance.memory ? … : null` branch.
    ///
    /// `performance.memory` is a non-standard Chromium field, so we read it via
    /// `js_sys::Reflect` rather than the `web-sys` `Memory` feature (which would
    /// force a web-sys/js-sys minor bump conflicting with the ort-web pin). A
    /// missing field or non-number value yields `None`.
    #[must_use]
    pub fn read_heap_bytes() -> Option<HeapBytes> {
        let perf = web_sys::window()?.performance()?;
        let mem = js_sys::Reflect::get(perf.as_ref(), &"memory".into()).ok()?;
        if mem.is_undefined() || mem.is_null() {
            return None;
        }
        let num = |k: &str| -> Option<f64> {
            let v = js_sys::Reflect::get(&mem, &k.into()).ok()?;
            let n = v.as_f64()?;
            n.is_finite().then_some(n)
        };
        Some(HeapBytes {
            used: num("usedJSHeapSize")?,
            total: num("totalJSHeapSize")?,
            limit: num("jsHeapSizeLimit")?,
        })
    }

    /// Monotonic `performance.now()` milliseconds for the `put` event's
    /// `now_ms`. Falls back to `0.0` if no `performance` is available (the
    /// resulting `last_step_ms` is then `0`, matching the JS first-put path).
    #[must_use]
    pub fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{LocalStorageSink, now_ms, read_heap_bytes};
