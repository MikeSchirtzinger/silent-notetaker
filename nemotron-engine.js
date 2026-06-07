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
 * Model artifacts (you host these; ~892 MB total, first load only, then cached):
 *   encoder.onnx (INT8, ~881 MB) · decoder_joint.onnx (INT8, ~11 MB) · tokenizer.model (~251 KB)
 * (The INT8 decoder's DynamicQuantizeLSTM contrib op is supported by
 * onnxruntime-web — verified in-browser 2026-06-05. The old fp32 decoder was a
 * mismatched checkpoint: no ITN, garbled dense audio. See registry/models.toml.)
 * Default base is `./crates/nemotron-asr/models/` (local dev). For a hosted build:
 *   window.__NEMOTRON_MODEL_BASE = 'https://<cdn>/.../';
 */

// The typed Nemotron surface now lives in silent-web's pkg (alongside the
// diarization + notes surfaces — one wasm binary). The model files still live
// under the nemotron-asr crate dir (local dev), overridable via the globals.
const DEFAULT_PKG_URL    = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;
const DEFAULT_MODEL_BASE = new URL('./crates/nemotron-asr/models/', import.meta.url).href;

// Hosted fallback: the deploy bundle does NOT ship the ~892 MB of model
// artifacts (Cloudflare Pages caps files at 25 MB) — they stream from the SAME
// pinned repo+revision as registry/models.toml (the source of truth for this
// pin; update BOTH together). Used only when the same-origin models dir is not
// genuinely served — see _resolveModelBase(): a static host's SPA fallback
// answers that path with index.html + 200, so a bare status check proves
// nothing.
const HF_MODEL_BASE =
  'https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/95df6c82aa796a3fc793f87633dcdb017ac12c07/';

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
    this._drainedSamples = 0; // samples decoded so far (see consumedSamples getter)
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

    const base = await this._resolveModelBase();
    // Encoder dominates the download (~881 MB) — stream it so we can show real progress.
    this.onStatus?.('Downloading Nemotron model (encoder ~881 MB, first load only)…', 5);
    const enc = await this._fetchBytes(base + 'encoder.onnx', 'encoder.onnx');
    const [dec, tok] = await Promise.all([
      this._fetchBytes(base + 'decoder_joint.onnx', 'decoder_joint.onnx'),
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
    this._drainedSamples = 0;
  }

  /**
   * Total samples drained into the wasm decoder so far. Because the host feeds
   * the ring buffer and this engine from the same capture callback, this maps
   * 1:1 onto ring-absolute positions — it is the position of the audio the
   * emitted text actually CAME FROM (±1 undecoded wasm-internal tail chunk,
   * ≤ ~0.8 s), unlike ring.writeAbs which tracks the live capture head and
   * runs 0.6–0.9 s+backlog ahead of the transcript. Speaker attribution uses
   * this so each sentence's audio slice aligns with its words.
   */
  get consumedSamples() { return this._drainedSamples || 0; }

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
      // Advance BEFORE dispatch so onText handlers reading consumedSamples see
      // the position including the chunk this text came from.
      this._drainedSamples = (this._drainedSamples || 0) + buf.length;
      if (raw != null) this._dispatch(JSON.parse(raw));
      if (final) break;   // final pass takes everything in one shot
    }
  }

  _emit(txt) {
    if (!this._firstTextAt) this._firstTextAt = performance.now();
    this.onText?.(txt);
  }

  /**
   * Resolve where the model artifacts actually live. An explicit override
   * (opts.modelBase / __NEMOTRON_MODEL_BASE) is honored verbatim. The
   * same-origin default is PROBED first: the local servers serve the real
   * models dir, but a static hosted deploy does not ship it, and its SPA
   * fallback answers the path with index.html + 200 — so require the probe
   * response to look like model bytes (ok AND not text/html) before trusting
   * it. Otherwise stream from the pinned HF repo. Loud either way.
   */
  async _resolveModelBase() {
    const base = this.modelBase.endsWith('/') ? this.modelBase : this.modelBase + '/';
    if (base !== DEFAULT_MODEL_BASE) return base;   // explicit override — honor verbatim
    try {
      const r = await fetch(base + 'tokenizer.model', { method: 'HEAD', cache: 'no-store' });
      if (r.ok && !/text\/html/i.test(r.headers.get('content-type') || '')) {
        console.log('[nemotron] model source: same-origin', base);
        return base;
      }
    } catch (_) { /* unreachable dir — fall through to HF */ }
    console.log('[nemotron] model source: same-origin models dir not served — streaming from pinned HF repo', HF_MODEL_BASE);
    return HF_MODEL_BASE;
  }

  async _fetchBytes(url, fileLabel) {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`fetch ${url}: ${r.status}`);
    // A static host's SPA fallback serves index.html with a 200 for paths that
    // don't exist — feeding HTML into the onnx session builder produces an
    // opaque downstream wedge. Fail HERE, loudly, instead.
    const ct = r.headers.get('content-type') || '';
    if (/text\/html/i.test(ct)) {
      throw new Error(`fetch ${url}: got ${ct} instead of model bytes (static-host SPA fallback?)`);
    }
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
