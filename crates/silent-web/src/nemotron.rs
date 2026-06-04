//! Wasm-bindgen Nemotron ASR surface (PRD Phase 3, Task w4; Appendix A rows 9,
//! 35). The Nemotron adapter migrated from `nemotron-engine.js`'s ad-hoc event
//! glue onto the typed `silent-core` event boundary.
//!
//! # What this wraps
//!
//! [`WasmNemotron`] owns a [`nemotron_asr::WasmAsr`] (the ort-web RNN-T engine)
//! and the **event-glue policy** that used to live as scattered callbacks in
//! `nemotron-engine.js`:
//!
//! - **load progress** (Appendix A row 9): per-file fetch progress and ready
//!   now emit [`silent_core::EngineEvent::LoadProgress`] / `Ready` instead of
//!   `onStatus(msg, pct)` strings invented ad-hoc in JS.
//! - **telemetry** (Appendix A row 35): the chunk-timing counters that JS held
//!   on the engine object (`_chunkCount`, `_totalChunkMs`, RTF, time-to-first-
//!   text) move here and emit [`silent_core::EngineEvent::Stats`] carrying a
//!   typed [`silent_core::EngineStats`], the SAME field set the PerfMonitor
//!   already reads.
//! - **transcript text**: each decoded chunk emits
//!   [`silent_core::EngineEvent::Partial`]; the end-of-stream tail emits
//!   [`silent_core::EngineEvent::Final`].
//!
//! The chunk feeding / decode itself is **unchanged** — [`WasmNemotron`] calls
//! [`nemotron_asr::WasmAsr::transcribe_chunk`] / `finalize` / `reset` exactly as
//! `nemotron-engine.js` did. Only the JS-facing event glue migrated. The buffer
//! drain loop (whole-chunk slicing) and the model-byte fetching stay in the thin
//! JS loader (`nemotron-engine.js`): fetching bytes and serializing audio off
//! the mic are host *execution*, not policy, and keeping them in JS preserves
//! the streaming hot-path measured baseline (no extra wasm round-trip per feed).
//!
//! # Event shape
//!
//! Methods return the `silent_core::EngineEvent` serde JSON
//! (`{ "tag": "...", "payload": ... }`, snake_case tags) — the same convention
//! as [`crate::diarization`] / [`crate::notes`]. The thin JS loader `JSON.parse`s
//! the event and dispatches it; it also exposes backward-compatible
//! `onStatus`/`onText`/`stats()` adapters derived from these typed events so the
//! index.html rendering path is pixel-identical.
//!
//! # wasm32-only
//!
//! Compiled only for `wasm32-unknown-unknown` (it pulls in `nemotron-asr`'s
//! ort-web engine). The native workspace build gates this module out (see
//! `lib.rs`); `cargo check --workspace` stays free of an ort-web link.

use nemotron_asr::WasmAsr;
use silent_core::events::{EngineEvent, EngineStats};
use silent_core::ids::TimeRange;

use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Serialize an [`EngineEvent`] to a `JsValue` (a JSON string the loader
/// `JSON.parse`s into a typed event). Matches the [`crate::diarization`] /
/// [`crate::notes`] convention exactly.
fn event_to_js(ev: &EngineEvent) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(ev).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

// ---------------------------------------------------------------------------
// Telemetry accumulator (the chunk-timing state, moved out of JS)
// ---------------------------------------------------------------------------

/// The latency counters that used to live on the `NemotronEngine` JS object
/// (`_chunkCount`, `_totalChunkMs`, `_lastChunkMs`, `_audioSecs`, …). Held here
/// so the [`EngineStats`] the PerfMonitor reads is computed by Rust policy, not
/// re-derived in JS. Wall-clock instants (`performance.now()`) stay in JS — the
/// Rust core has no clock (PRD R5 browser-free rule) — so the loader passes the
/// pre-computed `load_ms` and `ttft_ms` deltas in.
#[derive(Default)]
struct Telemetry {
    load_ms: u32,
    chunks: u32,
    total_chunk_ms: f64,
    last_chunk_ms: f64,
    audio_secs: f64,
    ttft_ms: u32,
    pending_samples: u32,
}

impl Telemetry {
    fn reset(&mut self) {
        // Keep `load_ms` — it is a one-time cost paid at load, not per session.
        let load_ms = self.load_ms;
        *self = Telemetry::default();
        self.load_ms = load_ms;
    }

    /// Record one decoded chunk's audio duration and bump the chunk count.
    /// Mirrors the JS `_drain` accounting (`_chunkCount++; _audioSecs += …`).
    /// The wall-clock decode cost is reported separately via
    /// [`record_decode_ms`](Telemetry::record_decode_ms) — it is only knowable
    /// after the decode `await` resolves, so the loader reports it post-await.
    fn record_chunk(&mut self, audio_secs: f64) {
        self.chunks += 1;
        self.audio_secs += audio_secs;
    }

    /// Record the wall-clock decode cost of the chunk that just finished
    /// (`_totalChunkMs += dt; _lastChunkMs = dt` in the JS `_drain`). Called by
    /// the loader after the decode `await`.
    fn record_decode_ms(&mut self, chunk_ms: f64) {
        self.total_chunk_ms += chunk_ms;
        self.last_chunk_ms = chunk_ms;
    }

    /// Build the typed [`EngineStats`] snapshot — byte-for-byte the same numbers
    /// the JS `stats()` method produced (rounded the same way), so the
    /// PerfMonitor row is unchanged.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "telemetry rounding to integer milliseconds mirrors the JS \
                  Math.round() the PerfMonitor already displayed; the values are \
                  small, non-negative latencies/counts well within u32"
    )]
    fn snapshot(&self) -> EngineStats {
        let avg = if self.chunks > 0 {
            self.total_chunk_ms / f64::from(self.chunks)
        } else {
            0.0
        };
        // RTF = processing-time / audio-duration. < 1.0 beats realtime. The JS
        // computed `(_totalChunkMs/1000)/_audioSecs` to 3 dp; do the same.
        let rtf = if self.audio_secs > 0.0 {
            ((self.total_chunk_ms / 1000.0) / self.audio_secs * 1000.0).round() / 1000.0
        } else {
            0.0
        };
        let audio_secs = (self.audio_secs * 100.0).round() / 100.0;
        EngineStats {
            load_ms: self.load_ms,
            chunks: self.chunks,
            avg_chunk_ms: avg.round() as u32,
            last_chunk_ms: self.last_chunk_ms.round() as u32,
            audio_secs: audio_secs as f32,
            rtf: rtf as f32,
            ttft_ms: self.ttft_ms,
            pending_samples: self.pending_samples,
        }
    }
}

// ---------------------------------------------------------------------------
// WasmNemotron — the typed event surface over WasmAsr
// ---------------------------------------------------------------------------

/// Browser-facing Nemotron ASR surface: a thin typed-event layer over
/// [`nemotron_asr::WasmAsr`].
///
/// # Lifecycle (mirrors the old `nemotron-engine.js`)
///
/// The thin JS loader fetches the three model files (host I/O), reporting fetch
/// progress through [`load_progress_event`](WasmNemotron::load_progress_event),
/// then builds the engine with [`create`](WasmNemotron::create) and emits
/// [`ready_event`](WasmNemotron::ready_event). It then feeds whole 56-frame
/// chunks: each [`transcribe_chunk`](WasmNemotron::transcribe_chunk) returns a
/// [`EngineEvent::Partial`] (text + a `Stats` follow-up is fetched via
/// [`stats_event`](WasmNemotron::stats_event)); the end-of-stream
/// [`finalize`](WasmNemotron::finalize) returns a [`EngineEvent::Final`].
///
/// The decode itself is the unchanged `WasmAsr`; this struct adds only the typed
/// event glue (Appendix A rows 9, 35) and the telemetry that used to live in JS.
#[wasm_bindgen]
pub struct WasmNemotron {
    asr: WasmAsr,
    telemetry: Telemetry,
    /// Absolute session-elapsed milliseconds at which the next emitted segment
    /// starts. The loader advances time by feeding the per-chunk audio duration;
    /// this lets `Partial`/`Final` carry a real [`TimeRange`] (the JS path
    /// stamped DOM elements with `Date.now()`, but the typed boundary wants a
    /// session-relative span).
    cursor_ms: u64,
    /// Whether any text has been emitted yet (drives the one-shot TTFT capture).
    emitted_text: bool,
}

#[wasm_bindgen]
impl WasmNemotron {
    /// Build the Nemotron engine from the three in-memory model artifacts (the
    /// thin JS loader fetched them, reporting progress via
    /// [`load_progress_event`](WasmNemotron::load_progress_event)).
    ///
    /// This is the unchanged [`nemotron_asr::WasmAsr::create`] under a typed
    /// wrapper — it initialises ort-web and commits both ONNX sessions. After it
    /// resolves the loader should warm up
    /// ([`warm_up`](WasmNemotron::warm_up)) then emit
    /// [`ready_event`](WasmNemotron::ready_event).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if `WasmAsr::create` fails (ort-web init, ONNX session
    /// build, or tokenizer parse).
    pub async fn create(
        encoder_onnx: &[u8],
        decoder_onnx: &[u8],
        tokenizer_model: &[u8],
    ) -> Result<WasmNemotron, JsError> {
        console_error_panic_hook::set_once();
        let asr = WasmAsr::create(encoder_onnx, decoder_onnx, tokenizer_model).await?;
        Ok(WasmNemotron {
            asr,
            telemetry: Telemetry::default(),
            cursor_ms: 0,
            emitted_text: false,
        })
    }

    /// Pay the one-time JIT / arena-growth cost up front so the user's first
    /// spoken words are not garbled (the `nemotron-engine.js` warm-up trick).
    /// Runs one synthetic 1.2 s chunk through the decode then resets state.
    ///
    /// `load_ms` is the wall-clock load+warm-up cost the JS loader measured
    /// (`performance.now()` delta); it is stored for the [`EngineStats`] the
    /// PerfMonitor reads. The Rust core has no clock, so the loader supplies it.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the warm-up decode fails (the loader logs and
    /// continues — a warm-up failure is non-fatal, matching the JS try/catch).
    #[wasm_bindgen(js_name = warmUp)]
    pub async fn warm_up(&mut self, load_ms: f64) -> Result<(), JsError> {
        // 19 200 samples = 1.2 s @ 16 kHz, the same synthetic warm-up chunk JS used.
        let warm = vec![0.0f32; 19_200];
        // A warm-up decode error is non-fatal (the JS path try/caught it). Swallow
        // it here so a transient warm-up hiccup never blocks Ready.
        let _ = self.asr.transcribe_chunk(&warm).await;
        self.asr.reset();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "load_ms is a small non-negative wall-clock delta in ms, \
                      well within u32; the f64 boundary is the JS number type"
        )]
        {
            self.telemetry.load_ms = load_ms.max(0.0).round() as u32;
        }
        Ok(())
    }

    /// Build the typed [`EngineEvent::Ready`] the loader emits once the engine is
    /// loaded and warmed up (Appendix A row 9 terminal event).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = readyEvent)]
    pub fn ready_event(&self) -> Result<JsValue, JsError> {
        event_to_js(&EngineEvent::Ready)
    }

    /// Reset all streaming + telemetry state for a fresh utterance/session
    /// (mirrors the JS `reset()`). Keeps the loaded ONNX sessions and the
    /// one-time `load_ms`; clears decode state, the time cursor, and the per-
    /// session chunk counters.
    pub fn reset(&mut self) {
        self.asr.reset();
        self.telemetry.reset();
        self.cursor_ms = 0;
        self.emitted_text = false;
    }

    /// Decode one audio chunk and return a typed [`EngineEvent::Partial`].
    ///
    /// This is the unchanged [`nemotron_asr::WasmAsr::transcribe_chunk`] decode
    /// wrapped with the event glue. The returned event's [`TimeRange`] spans the
    /// audio this chunk added. The wall-clock decode cost is reported separately
    /// by the loader via [`record_decode_ms`](WasmNemotron::record_decode_ms)
    /// after the `await` resolves (it cannot be known before).
    ///
    /// Returns `null` when the chunk decoded no text (the JS `if (txt) …` guard)
    /// — the loader emits nothing in that case, exactly as before. Even when
    /// `null`, the audio duration is still counted (a silent chunk still
    /// advances time and RTF's denominator).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the underlying decode fails.
    #[wasm_bindgen(js_name = transcribeChunk)]
    pub async fn transcribe_chunk(&mut self, samples: &[f32]) -> Result<JsValue, JsError> {
        // Decode is unchanged: delegate to WasmAsr.
        let text = self.asr.transcribe_chunk(samples).await?;

        // Telemetry: this chunk's audio duration @ 16 kHz (decode cost arrives
        // separately, post-await, via record_decode_ms).
        #[allow(
            clippy::cast_precision_loss,
            reason = "a fed chunk is a few thousand samples (250 ms @ 16 kHz = \
                      4000); usize → f64 is exact far below the 2^52 mantissa limit"
        )]
        let audio_secs = samples.len() as f64 / 16_000.0;
        self.telemetry.record_chunk(audio_secs);

        // Advance the session time cursor by the audio this chunk added.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "per-chunk audio duration in ms is small and non-negative"
        )]
        let span_ms = (audio_secs * 1000.0).round() as u64;
        let range = TimeRange::new(self.cursor_ms, self.cursor_ms + span_ms);
        self.cursor_ms += span_ms;

        if text.is_empty() {
            return Ok(JsValue::null());
        }
        self.emitted_text = true;
        event_to_js(&EngineEvent::Partial { text, range })
    }

    /// Drain the trailing partial chunk at end of stream and return a typed
    /// [`EngineEvent::Final`] (or `null` when nothing remained).
    ///
    /// This is the unchanged [`nemotron_asr::WasmAsr::finalize`] decode wrapped
    /// with the event glue. The returned event's [`TimeRange`] spans the tail
    /// audio. The decode cost is reported via
    /// [`record_decode_ms`](WasmNemotron::record_decode_ms) post-await.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the underlying tail decode fails.
    pub async fn finalize(&mut self) -> Result<JsValue, JsError> {
        let text = self.asr.finalize().await?;
        self.telemetry.record_chunk(0.0);
        let range = TimeRange::new(self.cursor_ms, self.cursor_ms);
        if text.is_empty() {
            return Ok(JsValue::null());
        }
        self.emitted_text = true;
        event_to_js(&EngineEvent::Final { text, range })
    }

    /// Record the wall-clock decode cost (ms) of the chunk that just finished.
    /// A cheap synchronous setter the loader calls right after each decode
    /// `await` resolves (the cost is only knowable then). Mirrors the JS
    /// `_drain`'s `_totalChunkMs += dt; _lastChunkMs = dt`.
    #[wasm_bindgen(js_name = recordDecodeMs)]
    pub fn record_decode_ms(&mut self, chunk_ms: f64) {
        self.telemetry.record_decode_ms(chunk_ms.max(0.0));
    }

    /// Build the typed [`EngineEvent::Stats`] snapshot (Appendix A row 35) the
    /// PerfMonitor reads. The loader calls this on its sampling tick, supplying
    /// the two clock-derived deltas the Rust core cannot compute itself
    /// (`ttft_ms` = first-audio→first-text, and the live feed-buffer backlog
    /// `pending_samples`).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = statsEvent)]
    pub fn stats_event(&mut self, ttft_ms: f64, pending_samples: f64) -> Result<JsValue, JsError> {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "ttft_ms and the pending-sample backlog are small non-negative \
                      JS numbers; the f64 boundary is the JS number type"
        )]
        {
            self.telemetry.ttft_ms = ttft_ms.max(0.0).round() as u32;
            self.telemetry.pending_samples = pending_samples.max(0.0) as u32;
        }
        let ev = EngineEvent::Stats(self.telemetry.snapshot());
        event_to_js(&ev)
    }
}

// ---------------------------------------------------------------------------
// Free function — model-download progress (Appendix A row 9)
// ---------------------------------------------------------------------------

/// Build a typed [`EngineEvent::LoadProgress`] without an engine instance.
///
/// The encoder (~881 MB) is fetched and streamed by the JS loader BEFORE the
/// engine is built (the engine is built *from* those bytes), so its progress
/// events cannot come from a [`WasmNemotron`] method — they come from this free
/// function instead. The smaller files reuse it too, so the row-9 progress
/// stream is produced entirely by silent-web (never hand-rolled in JS).
///
/// `loaded`/`total` are byte counts (the loader reads them from the fetch
/// `content-length` + the stream reader); `total == 0` signals an unknown
/// length, exactly as the typed contract specifies.
///
/// # Errors
///
/// Returns a `JsError` only on JSON serialization failure.
#[wasm_bindgen(js_name = nemotronLoadProgressEvent)]
pub fn nemotron_load_progress_event(
    file: &str,
    loaded: f64,
    total: f64,
) -> Result<JsValue, JsError> {
    // Byte counts are non-negative and arrive as JS numbers; clamp to 0 and
    // truncate to the u64 the typed event carries.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "byte counts from fetch content-length are non-negative and far \
                  below u64::MAX; the f64 boundary is the JS number type"
    )]
    let ev = EngineEvent::LoadProgress {
        file: file.to_owned(),
        loaded: loaded.max(0.0) as u64,
        total: total.max(0.0) as u64,
    };
    event_to_js(&ev)
}
