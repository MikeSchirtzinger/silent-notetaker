/**
 * nemotron-engine.js — thin loader for the Nemotron in-browser ASR engine,
 * driven over the typed `silent-core` event boundary.
 *
 * Migration (PRD Phase 3, Task w4; Appendix A rows 9, 35): the event glue moved
 * out of this file and into Rust. This loader used to inline ~250 lines that
 * invented its own `onStatus(msg, pct)` / `onText(frag)` callbacks and computed
 * `stats()` in JS. Those now flow as TYPED `silent_core::EngineEvent`s
 * (`LoadProgress` / `Ready` / `Partial` / `Final` / `Stats`) produced by the
 * `WasmNemotron` surface in `crates/silent-web` (the same wasm-pack `pkg/` the
 * diarization + notes engines load — one wasm binary, now three surfaces).
 *
 * What stays here (host execution, not policy — deliberately NOT migrated):
 *   - fetching the three model files (a host I/O concern; the Rust core is
 *     browser-free by contract). Progress bytes feed `WasmNemotron.loadProgressEvent`.
 *   - the feed-buffer drain loop (whole-56-frame-chunk slicing) and the
 *     ort-web thread-count trap. Keeping the per-feed drain in JS preserves the
 *     measured streaming hot-path baseline (no extra wasm round-trip per feed).
 *   - the `performance.now()` wall-clock instants (the Rust core has no clock):
 *     this loader measures load_ms / chunk_ms / ttft_ms and hands the deltas to
 *     Rust, which owns the telemetry aggregation the PerfMonitor reads.
 *
 * The decode itself is the UNCHANGED `nemotron-asr` engine — `WasmNemotron`
 * calls `WasmAsr::transcribe_chunk` / `finalize` / `reset` verbatim.
 *
 * Backward-compatible public API: this class still exposes `onStatus` / `onText`
 * / `load` / `reset` / `feed` / `finalize` / `stats`, with byte-identical shapes,
 * so `index.html` is unchanged. Internally each is now derived from the typed
 * events (a `LoadProgress` → the same `onStatus(msg, pct)` string the UI rendered
 * before; a `Partial`/`Final` → the same `onText(fragment)`; a `Stats` → the same
 * `stats()` object). A new `onEvent(event)` hook is also exposed for consumers
 * that want the raw typed stream.
 *
 * Model artifacts (you host these; ~917 MB total, first load only, then cached):
 *   encoder.onnx (INT8, ~881 MB) · decoder_joint_fp32.onnx (~36 MB) · tokenizer.model (~251 KB)
 * Default base is `./crates/nemotron-asr/models/` (local dev). For a hosted build:
 *   window.__NEMOTRON_MODEL_BASE = 'https://<cdn>/.../';
 */

// The typed Nemotron surface now lives in silent-web's pkg (alongside the
// diarization + notes surfaces — one wasm binary). The model files still live
// under the nemotron-asr crate dir (local dev), overridable via the globals.
const DEFAULT_PKG_URL    = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;
const DEFAULT_MODEL_BASE = new URL('./crates/nemotron-asr/models/', import.meta.url).href;

// raiseOrtWasmThreads now lives in the permanent ort-web-loader.js module so
// any future ort-web host (TitaNet, Whisper-ort, …) can share the same
// thread-count trap without duplicating it. Imported here for backward compat.
import { raiseOrtWasmThreads } from './apps/web/js/ort-web-loader.js';

// How much audio to buffer before each transcribe_chunk call — the dominant lever on
// *perceived* latency. 250 ms feeds ⇒ a chunk decodes ~every 560 ms of speech, ~0.6-0.9 s
// behind live. The crate's EDGE_GUARD_FRAMES fix means feed size only sets how promptly
// whole clean chunks are noticed (re-validated against the golden clip 2026-06-04).
const FEED_SAMPLES = 4000;    // 250 ms @ 16 kHz

// Shared, idempotent module load (the diarization + notes engines load the same pkg URL,
// so the browser module cache returns the same instance; `mod.default()` is a no-op after
// the first init). Guard it so a NemotronEngine created standalone still works.
let _modPromise = null;
function _loadModule(pkgUrl) {
  if (_modPromise) return _modPromise;
  _modPromise = (async () => {
    const mod = await import(/* @vite-ignore */ pkgUrl);
    await mod.default();   // initialises the wasm binary (no-op if already done)
    return mod;
  })();
  return _modPromise;
}

export class NemotronEngine {
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.modelBase   = opts.modelBase || w.__NEMOTRON_MODEL_BASE || DEFAULT_MODEL_BASE;
    this.pkgUrl      = opts.pkgUrl    || w.__SILENT_WEB_PKG_URL   || w.__DIARIZATION_PKG_URL || w.__NEMOTRON_PKG_URL || DEFAULT_PKG_URL;
    this.feedSamples = opts.feedSamples || w.__NEMOTRON_FEED_SAMPLES || FEED_SAMPLES;
    // Leave 2 cores for the page + Qwen worker; cap at 8 (threads beyond the P-cores
    // regress on ort's spin-wait pool). Override via opts.numThreads for A/B runs.
    this.numThreads  = opts.numThreads || w.__NEMOTRON_THREADS
                    || Math.min(8, Math.max(1, ((typeof navigator !== 'undefined' && navigator.hardwareConcurrency) || 4) - 2));

    // Backward-compatible callbacks (unchanged shapes — index.html consumes these).
    this.onStatus = null;   // (message: string|null, pct: number|null) => void
    this.onText   = null;   // (fragment: string) => void  — incremental text emitted this chunk
    // New: the raw typed silent_core::EngineEvent stream (for consumers that want it).
    this.onEvent  = null;   // (event: { tag, payload }) => void

    this.engine = null;     // WasmNemotron instance
    this._mod   = null;
    this._pending = [];     // accumulated f32 samples awaiting a whole chunk
    this._chain = Promise.resolve();   // serializes transcribe_chunk so state never overlaps

    // ── latency telemetry: only the wall-clock instants stay in JS (the Rust core has no
    //    clock); the AGGREGATION (chunks/avg/rtf) now lives in WasmNemotron.
    this._startedAt = 0;        // performance.now() of first fed sample (per session)
    this._firstTextAt = 0;      // performance.now() when first text was emitted
    // Last typed Stats snapshot (snake_case from EngineStats), refreshed on each stats() call.
    this._lastStats = null;
  }

  /** Dispatch a typed EngineEvent: forward it raw, then translate to the legacy callbacks. */
  _dispatch(event) {
    if (!event) return;
    this.onEvent?.(event);
    switch (event.tag) {
      case 'load_progress': {
        // Re-derive the SAME status string + pct the UI rendered before. Encoder
        // dominates (~881 MB); map its byte fraction onto the 5-75% band, others to 80%.
        const p = event.payload || {};
        if (p.total > 0 && /encoder/i.test(p.file)) {
          const frac = Math.min(1, p.loaded / p.total);
          this.onStatus?.(`Downloading Nemotron encoder… ${Math.round(frac * 100)}%`, 5 + Math.round(frac * 70));
        }
        break;
      }
      case 'ready':
        this.onStatus?.('Nemotron ready — streaming transcription active (CPU/WASM, GPU free)', 100);
        break;
      case 'partial':
      case 'final': {
        const txt = (event.payload && event.payload.text) || '';
        if (txt) this._emit(txt);
        break;
      }
      case 'stats':
        this._lastStats = event.payload;   // EngineStats (snake_case)
        break;
      case 'warning':
        console.warn('[nemotron]', (event.payload && event.payload.message) || event.payload);
        break;
      default:
        // #[non_exhaustive]: an unknown additive tag is forwarded via onEvent and ignored here.
        break;
    }
  }

  async load() {
    const tLoad = performance.now();
    this.onStatus?.('Initializing Nemotron ASR (WASM)…', 3);

    this._mod = await _loadModule(this.pkgUrl);
    const { WasmNemotron } = this._mod;

    const base = this.modelBase.endsWith('/') ? this.modelBase : this.modelBase + '/';
    // Encoder dominates the download (~881 MB) — stream it so we can show real progress.
    this.onStatus?.('Downloading Nemotron model (encoder ~881 MB, first load only)…', 5);
    const enc = await this._fetchBytes(base + 'encoder.onnx', 'encoder.onnx');
    const [dec, tok] = await Promise.all([
      this._fetchBytes(base + 'decoder_joint_fp32.onnx', 'decoder_joint_fp32.onnx'),
      this._fetchBytes(base + 'tokenizer.model', 'tokenizer.model'),
    ]);

    this.onStatus?.('Building onnxruntime-web sessions…', 82);
    raiseOrtWasmThreads(this.numThreads);
    this.engine = await WasmNemotron.create(enc, dec, tok);
    const w = (typeof window !== 'undefined') ? window : {};
    console.log(`[nemotron] ort-web wasm threads = ${w.ort?.env?.wasm?.numThreads ?? '(default)'}`
      + ` (requested ${this.numThreads}, crossOriginIsolated=${!!w.crossOriginIsolated})`);

    // Warm-up: pay the wasm JIT tier-up + ort arena growth now (not on the user's first
    // spoken words). The Rust wrapper runs the synthetic chunk + resets decode state; we
    // hand it the measured load+warm-up wall-clock so the typed Stats carry the right load_ms.
    this.onStatus?.('Warming up Nemotron…', 94);
    const loadMs = performance.now() - tLoad;
    await this.engine.warmUp(loadMs);
    console.log(`[nemotron] load + warm-up ${Math.round(loadMs)} ms`);

    // Ready is a typed event now — dispatch it (drives the same onStatus(…,100)).
    this._dispatch(JSON.parse(this.engine.readyEvent()));
    return this;
  }

  /** Reset all streaming state for a fresh utterance/session. */
  reset() {
    this.engine?.reset();
    this._pending.length = 0;
    this._chain = Promise.resolve();
    this._startedAt = 0; this._firstTextAt = 0;
    this._lastStats = null;
  }

  /** Feed 16 kHz mono Float32 samples. Buffers, then drains whole chunks single-file. */
  feed(samples) {
    if (!this.engine || !samples || !samples.length) return;
    if (!this._startedAt) this._startedAt = performance.now();
    for (let i = 0; i < samples.length; i++) this._pending.push(samples[i]);
    this._kick(false);
  }

  /** Drain the buffer + decode the trailing partial chunk. Call once at end of stream. */
  async finalize() {
    this._kick(true);
    await this._chain;                       // ensure all queued chunks have decoded
    if (!this.engine) return '';
    const t0 = performance.now();
    const raw = await this.engine.finalize();
    this.engine.recordDecodeMs(performance.now() - t0);
    if (raw == null) return '';
    const event = JSON.parse(raw);
    this._dispatch(event);
    return (event.payload && event.payload.text) || '';
  }

  /**
   * Latency snapshot for benchmarking / on-screen readout. Same shape index.html's
   * PerfMonitor reads (rtf / avgChunkMs / lastChunkMs / pendingSamples / chunks / …),
   * now sourced from the typed EngineStats the Rust wrapper aggregates.
   */
  stats() {
    const ttft = (this._firstTextAt && this._startedAt) ? Math.round(this._firstTextAt - this._startedAt) : 0;
    if (this.engine) {
      // Refresh the typed snapshot, passing the two clock-derived deltas Rust can't compute.
      this._dispatch(JSON.parse(this.engine.statsEvent(ttft, this._pending.length)));
    }
    const s = this._lastStats;
    if (!s) {
      return { loadMs: 0, chunks: 0, avgChunkMs: 0, lastChunkMs: 0, audioSecs: 0, rtf: 0, timeToFirstTextMs: ttft, pendingSamples: this._pending.length };
    }
    // Map the typed (snake_case) EngineStats back to the camelCase the UI already reads.
    return {
      loadMs: s.load_ms,
      chunks: s.chunks,
      avgChunkMs: s.avg_chunk_ms,
      lastChunkMs: s.last_chunk_ms,
      audioSecs: s.audio_secs,
      rtf: s.rtf,
      timeToFirstTextMs: s.ttft_ms,
      pendingSamples: s.pending_samples,
    };
  }

  // ── internals ──
  _kick(final) {
    this._chain = this._chain.then(() => this._drain(final)).catch((e) => {
      console.warn('[nemotron] chunk decode error', e && e.message || e);
    });
  }

  async _drain(final) {
    if (!this.engine) return;
    while (this._pending.length >= this.feedSamples || (final && this._pending.length > 0)) {
      const take = final ? this._pending.length : this.feedSamples;
      const c = this._pending.splice(0, take);
      const buf = Float32Array.from(c);
      // transcribe_chunk returns a typed Partial event (or null for an empty chunk).
      // Measure the decode cost around the await and report it to Rust after (the
      // cost is only knowable once the await resolves), so Rust owns the aggregation.
      const t0 = performance.now();
      const raw = await this.engine.transcribeChunk(buf);
      this.engine.recordDecodeMs(performance.now() - t0);
      if (raw != null) this._dispatch(JSON.parse(raw));
      if (final) break;   // final pass takes everything in one shot
    }
  }

  _emit(txt) {
    if (!this._firstTextAt) this._firstTextAt = performance.now();
    this.onText?.(txt);
  }

  async _fetchBytes(url, fileLabel) {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`fetch ${url}: ${r.status}`);
    const len = +(r.headers.get('content-length') || 0);
    const label = fileLabel || url;
    // The LoadProgress events flow through silent-web (the free wasm function), produced
    // even though the engine isn't built yet during the encoder download (it's built FROM
    // these bytes). The module is loaded before any fetch, so the function is available.
    const progress = (loaded, total) =>
      this._dispatch(JSON.parse(this._mod.nemotronLoadProgressEvent(label, loaded, total)));
    // Stream the encoder so LoadProgress events fire incrementally; small files in one shot.
    const wantProgress = /encoder/i.test(label) && r.body && len;
    if (!wantProgress) {
      const bytes = new Uint8Array(await r.arrayBuffer());
      progress(bytes.length, len || bytes.length);   // single terminal event
      return bytes;
    }
    const reader = r.body.getReader();
    const out = new Uint8Array(len);
    let off = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      out.set(value, off);
      off += value.length;
      progress(off, len);
    }
    return out;
  }
}

export default NemotronEngine;
