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
//! The decode itself is **unchanged** — [`WasmNemotron`] calls
//! [`nemotron_asr::WasmAsr::transcribe_chunk`] / `finalize` / `reset` exactly as
//! before. [`WasmNemotronBacklog`] now owns the queue bound, spill ordering, and
//! overload behavior through the native-tested
//! [`silent_inference::nemotron_backlog`] policy. JavaScript retains only the
//! browser hands: microphone delivery, OPFS reads/writes, fetch, and the async
//! call executor.
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
use silent_inference::nemotron_backlog::{NemotronBacklog, NemotronBacklogConfig};

use serde::Serialize;
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

fn value_to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(value).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

// ---------------------------------------------------------------------------
// WasmNemotronBacklog — bounded audio queue + OPFS spill ordering
// ---------------------------------------------------------------------------

/// Browser boundary for the pure-Rust bounded Nemotron backlog policy.
///
/// The browser host supplies captured PCM and executes the returned commands:
/// staged segments are written to temporary origin-private files; spool reads
/// and in-memory chunks are passed to [`WasmNemotron::transcribe_chunk`].
/// Capacity, ordering, acknowledgements, and terminal overload decisions stay
/// in Rust.
#[wasm_bindgen]
pub struct WasmNemotronBacklog {
    policy: NemotronBacklog,
}

#[wasm_bindgen]
impl WasmNemotronBacklog {
    /// Create the shipping 60-second resident / 30-minute disk-bounded policy.
    ///
    /// `chunk_samples` is the decoder feed selected by the host (normally 4,000
    /// samples = 250 ms).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if `chunk_samples` is zero.
    #[wasm_bindgen(constructor)]
    pub fn new(chunk_samples: u32) -> Result<WasmNemotronBacklog, JsError> {
        console_error_panic_hook::set_once();
        let config = NemotronBacklogConfig::shipping_with_chunk_samples(chunk_samples as usize);
        let policy = NemotronBacklog::new(config).map_err(to_js_err)?;
        Ok(Self { policy })
    }

    /// Append captured 16 kHz Float32 PCM and return the bounded queue snapshot.
    ///
    /// # Errors
    ///
    /// Throws on the sticky RAM/disk bound or a host protocol violation. The JS
    /// executor treats this as fatal and stops capture loudly.
    #[wasm_bindgen(js_name = pushSamples)]
    pub fn push_samples(&mut self, samples: &[f32]) -> Result<JsValue, JsError> {
        let snapshot = self.policy.push_samples(samples).map_err(to_js_err)?;
        value_to_js(&snapshot)
    }

    /// Describe the oldest batch waiting to cross into the OPFS executor.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = stagedSpill)]
    pub fn staged_spill(&self) -> Result<JsValue, JsError> {
        value_to_js(&self.policy.staged_spill())
    }

    /// Take the exact staged batch named by [`staged_spill`](Self::staged_spill).
    ///
    /// # Errors
    ///
    /// Throws when the host requests an unknown or out-of-order spill id.
    #[wasm_bindgen(js_name = takeStagedSpill)]
    pub fn take_staged_spill(&mut self, id: u32) -> Result<Box<[f32]>, JsError> {
        let batch = self.policy.take_staged_spill(id).map_err(to_js_err)?;
        Ok(batch.samples.into_boxed_slice())
    }

    /// Report that the temporary OPFS file is durably closed and readable.
    ///
    /// # Errors
    ///
    /// Throws for an unknown/out-of-state spill id.
    #[wasm_bindgen(js_name = markSpillReady)]
    pub fn mark_spill_ready(&mut self, id: u32) -> Result<JsValue, JsError> {
        let snapshot = self.policy.mark_spill_ready(id).map_err(to_js_err)?;
        value_to_js(&snapshot)
    }

    /// Make an OPFS write failure terminal. Returns the exact error string the
    /// UI should surface.
    #[wasm_bindgen(js_name = markSpillFailed)]
    #[must_use]
    pub fn mark_spill_failed(&mut self, id: u32, message: &str) -> String {
        self.policy.mark_spill_failed(id, message).to_string()
    }

    /// Return the next ordered decode command.
    ///
    /// # Errors
    ///
    /// Throws on a sticky bound failure or acknowledgement protocol violation.
    #[wasm_bindgen(js_name = nextDecode)]
    pub fn next_decode(&self, final_pass: bool) -> Result<JsValue, JsError> {
        let action = self.policy.next_decode(final_pass).map_err(to_js_err)?;
        value_to_js(&action)
    }

    /// Copy the commanded in-memory chunk to the JS executor while retaining a
    /// bounded acknowledgement copy.
    ///
    /// # Errors
    ///
    /// Throws when the count/source does not match [`next_decode`](Self::next_decode).
    #[wasm_bindgen(js_name = takeMemory)]
    pub fn take_memory(&mut self, count: u32) -> Result<Box<[f32]>, JsError> {
        let samples = self.policy.take_memory(count as usize).map_err(to_js_err)?;
        Ok(samples.into_boxed_slice())
    }

    /// Commit a successfully decoded in-memory chunk.
    ///
    /// # Errors
    ///
    /// Throws for a mismatched acknowledgement.
    #[wasm_bindgen(js_name = ackMemory)]
    pub fn ack_memory(&mut self, count: u32) -> Result<JsValue, JsError> {
        let snapshot = self.policy.ack_memory(count as usize).map_err(to_js_err)?;
        value_to_js(&snapshot)
    }

    /// Put a failed in-memory decode back at the exact queue front.
    ///
    /// # Errors
    ///
    /// Throws when no memory chunk is awaiting acknowledgement.
    #[wasm_bindgen(js_name = nackMemory)]
    pub fn nack_memory(&mut self) -> Result<JsValue, JsError> {
        let snapshot = self.policy.nack_memory().map_err(to_js_err)?;
        value_to_js(&snapshot)
    }

    /// Commit a successfully decoded OPFS range. Returns `true` when the host
    /// may delete the completed spill file.
    ///
    /// # Errors
    ///
    /// Throws for an out-of-order id/count.
    #[wasm_bindgen(js_name = ackSpill)]
    pub fn ack_spill(&mut self, id: u32, count: u32) -> Result<bool, JsError> {
        self.policy.ack_spill(id, count as usize).map_err(to_js_err)
    }

    /// Current bounded queue telemetry.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn snapshot(&self) -> Result<JsValue, JsError> {
        value_to_js(&self.policy.snapshot())
    }

    /// Audio successfully handed through the decoder in this session.
    #[wasm_bindgen(getter, js_name = consumedSamples)]
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "session PCM counts stay far below JavaScript's exact 2^53 integer limit"
    )]
    pub fn consumed_samples(&self) -> f64 {
        self.policy.snapshot().consumed_samples as f64
    }

    /// Total samples waiting in memory or OPFS.
    #[wasm_bindgen(getter, js_name = pendingSamples)]
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "shipping policy caps pending audio below 30 minutes at 16 kHz"
    )]
    pub fn pending_samples(&self) -> u32 {
        self.policy.snapshot().pending_samples as u32
    }

    /// Clear all per-session audio. Spill ids remain monotonic so late browser
    /// callbacks cannot alias new-session files.
    pub fn reset(&mut self) {
        self.policy.reset();
    }
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

    /// Build the Nemotron engine, loading the `ort-web` runtime from a
    /// same-origin vendored base URL (e.g. `"./vendor/ort-web/1.24.3/"`).
    ///
    /// The R6 vendoring entry point (Task K2): identical to [`create`] except the
    /// onnxruntime-web runtime is fetched same-origin, which keeps `cdn.pyke.io`
    /// out of the page CSP `connect-src`/`script-src`. The thin JS loader
    /// (`nemotron-engine.js`) passes the vendored base when one is configured and
    /// falls back to [`create`] (CDN) otherwise.
    ///
    /// [`create`]: WasmNemotron::create
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the vendored `WasmAsr::create_with_dist` fails
    /// (ort-web init, ONNX session build, or tokenizer parse).
    #[wasm_bindgen(js_name = createWithDist)]
    pub async fn create_with_dist(
        encoder_onnx: &[u8],
        decoder_onnx: &[u8],
        tokenizer_model: &[u8],
        dist_base_url: &str,
    ) -> Result<WasmNemotron, JsError> {
        console_error_panic_hook::set_once();
        let asr =
            WasmAsr::create_with_dist(encoder_onnx, decoder_onnx, tokenizer_model, dist_base_url)
                .await?;
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
