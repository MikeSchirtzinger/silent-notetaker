/**
 * nemotron-engine.js — swappable in-browser ASR engine (NVIDIA Nemotron streaming 0.6B).
 *
 * This is the thin, hand-written ES-module wrapper around the wasm-pack build of the
 * `nemotron-asr` Rust crate (`nemotron-asr/pkg/`). It exists so `index.html` can adopt
 * the engine with a tiny merge surface: it `import()`s this module and drives a clean
 * `NemotronEngine` instead of inlining ~600 lines of WASM glue + chunk plumbing.
 *
 * The engine runs entirely on CPU (ort-web → onnxruntime-web WASM, multithreaded when the
 * page is cross-origin isolated). It NEVER touches the GPU — which is the whole point of
 * pairing it with Qwen-on-WebGPU: the ASR leaves the GPU free for the "smart questions"
 * model. Contrast Voxtral, which owns the GPU.
 *
 * Wire-up (in index.html):
 *   const { NemotronEngine } = await import('./nemotron-engine.js');
 *   const eng = new NemotronEngine();
 *   eng.onStatus = (msg, pct) => updateUI(msg, pct);
 *   eng.onText   = (frag)     => appendTranscript(frag);   // incremental text per chunk
 *   await eng.load();          // fetch model + build ort sessions
 *   eng.reset();
 *   // feed 16 kHz mono Float32 frames as they arrive from the mic:
 *   eng.feed(samples);
 *   // at stop:
 *   await eng.finalize();      // drains the buffer + decodes the trailing < 560ms tail
 *
 * Model artifacts (you host these; ~917 MB total, first load only, then browser-cached):
 *   encoder.onnx (INT8, ~881 MB) · decoder_joint_fp32.onnx (~36 MB) · tokenizer.model (~251 KB)
 * Default base is `./nemotron-asr/models/` (local dev). For a hosted build, set
 *   window.__NEMOTRON_MODEL_BASE = 'https://<cdn>/.../';   // e.g. a HuggingFace repo, like titanet.onnx
 * before the engine loads.
 */

// Paths follow the crate's home after the Phase-1 workspace move
// (`nemotron-asr/` → `crates/nemotron-asr/`). The wasm-pack output (`pkg/`) and
// the local-dev model dir (`models/`, only used when `__NEMOTRON_MODEL_BASE` is
// unset) now live under `crates/nemotron-asr/`. Resolved relative to this
// module's URL, so it works wherever the app shell is served from.
const DEFAULT_MODEL_BASE = new URL('./crates/nemotron-asr/models/', import.meta.url).href;
const DEFAULT_PKG_URL    = new URL('./crates/nemotron-asr/pkg/nemotron_asr.js', import.meta.url).href;

/* onnxruntime-web defaults to min(4, ceil(hardwareConcurrency/2)) WASM threads — 4 on a
   10-core M1 Pro, leaving half the performance cores idle under the INT8 encoder. The
   `ort` global is created by ort-web's CDN script *inside* WasmAsr.create(), so to raise
   the count before the wasm runtime initializes (first session build) we trap the global's
   assignment. Threads need SharedArrayBuffer, so this no-ops without cross-origin
   isolation — ort would fall back to 1 anyway. */
function raiseOrtWasmThreads(desired) {
  if (typeof window === 'undefined' || !window.crossOriginIsolated || !(desired > 1)) return;
  // `env.wasm` may be populated after the global is assigned (bundle-dependent), and
  // it must be set before the FIRST session build initializes the wasm runtime — so
  // retry briefly instead of assuming the assignment carries a finished object.
  const apply = (ort) => {
    if (!ort) return;
    let tries = 0;
    const tick = () => {
      try { if (ort.env && ort.env.wasm) { ort.env.wasm.numThreads = desired; return; } } catch (_) { return; }
      if (++tries < 100) setTimeout(tick, 10);
    };
    tick();
  };
  if (window.ort) { apply(window.ort); return; }
  // Seed with a benign empty object: ort-web's loader probes `window.ort[initSymbol]`
  // whenever the property exists, and an accessor returning undefined would make that
  // probe throw (learned the hard way — it wedges load() at "Building sessions").
  let val = {};
  try {
    Object.defineProperty(window, 'ort', {
      configurable: true,
      enumerable: true,
      get() { return val; },
      set(v) { val = v; apply(v); },
    });
  } catch (_) { /* defineProperty refused — live with ort's default thread count */ }
}
// How much audio to buffer before each transcribe_chunk call — the dominant lever on
// *perceived* latency. History: 500 ms feeds used to merge word boundaries ("intelligence
// is" → "intelligences") and once stalled the decoder permanently after leading silence,
// so this sat at 16000 (1 s). Root cause found 2026-06-04: the crate was consuming mel
// frames from the buffer's right-edge zero-padding zone (synthetic zeros standing in for
// audio that hadn't arrived yet), corrupting the decode AND the carried encoder cache.
// Fixed in-crate by EDGE_GUARD_FRAMES (constants.rs) — the engine now only decodes whole
// CLEAN 56-frame chunks, so the feed size merely sets how promptly chunks are noticed.
// 250 ms feeds ⇒ a chunk decodes ~every 560 ms of speech, ~0.6-0.9 s behind live instead
// of ~1.5 s at 1 s feeds. Re-validated against the golden clip + silence-gap tests at
// 250 ms (2026-06-04). Override via opts.feedSamples.
const FEED_SAMPLES       = 4000;    // 250 ms @ 16 kHz

export class NemotronEngine {
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.modelBase   = opts.modelBase || w.__NEMOTRON_MODEL_BASE || DEFAULT_MODEL_BASE;
    this.pkgUrl      = opts.pkgUrl    || w.__NEMOTRON_PKG_URL    || DEFAULT_PKG_URL;
    this.feedSamples = opts.feedSamples || w.__NEMOTRON_FEED_SAMPLES || FEED_SAMPLES;
    // Leave 2 cores for the page + Qwen worker; cap at 8 (threads beyond the P-cores
    // regress on ort's spin-wait pool). Override via opts.numThreads for A/B runs.
    this.numThreads  = opts.numThreads || w.__NEMOTRON_THREADS
                    || Math.min(8, Math.max(1, ((typeof navigator !== 'undefined' && navigator.hardwareConcurrency) || 4) - 2));

    this.onStatus = null;   // (message: string|null, pct: number|null) => void
    this.onText   = null;   // (fragment: string) => void  — incremental text emitted this chunk

    this.asr = null;
    this._WasmAsr = null;
    this._pending = [];     // accumulated f32 samples awaiting a whole chunk
    this._chain = Promise.resolve();   // serializes transcribe_chunk so state never overlaps

    // ── latency telemetry (the "is Nemotron laggy vs Voxtral?" instrument) ──
    this._chunkCount = 0;
    this._totalChunkMs = 0;
    this._lastChunkMs = 0;
    this._audioSecs = 0;
    this._startedAt = 0;        // performance.now() of first fed sample (per session)
    this._firstTextAt = 0;      // performance.now() when first text was emitted
    this._loadMs = 0;
  }

  async load() {
    const tLoad = performance.now();
    this.onStatus?.('Initializing Nemotron ASR (WASM)…', 3);
    if (!this._WasmAsr) {
      const mod = await import(/* @vite-ignore */ this.pkgUrl);
      await mod.default();                 // init the wasm-bindgen module
      this._WasmAsr = mod.WasmAsr;
    }

    const base = this.modelBase.endsWith('/') ? this.modelBase : this.modelBase + '/';
    // Encoder dominates the download (~881 MB) — stream it so we can show real progress.
    this.onStatus?.('Downloading Nemotron model (encoder ~881 MB, first load only)…', 5);
    const enc = await this._fetchBytes(base + 'encoder.onnx', (frac) => {
      this.onStatus?.(`Downloading Nemotron encoder… ${Math.round(frac * 100)}%`, 5 + Math.round(frac * 70));
    });
    const [dec, tok] = await Promise.all([
      this._fetchBytes(base + 'decoder_joint_fp32.onnx'),
      this._fetchBytes(base + 'tokenizer.model'),
    ]);

    this.onStatus?.('Building onnxruntime-web sessions…', 82);
    raiseOrtWasmThreads(this.numThreads);
    this.asr = await this._WasmAsr.create(enc, dec, tok);
    const w = (typeof window !== 'undefined') ? window : {};
    console.log(`[nemotron] ort-web wasm threads = ${w.ort?.env?.wasm?.numThreads ?? '(default)'}`
      + ` (requested ${this.numThreads}, crossOriginIsolated=${!!w.crossOriginIsolated})`);

    // Warm-up: the first inferences pay wasm JIT tier-up + onnxruntime arena growth —
    // previously paid on the user's first spoken words (and a big part of "it garbled
    // the start"). One synthetic 1.2 s chunk exercises every encoder/decoder kernel;
    // reset() then clears all decode state, so accuracy is unaffected.
    this.onStatus?.('Warming up Nemotron…', 94);
    const tWarm = performance.now();
    try { await this.asr.transcribe_chunk(new Float32Array(19200)); }
    catch (e) { console.warn('[nemotron] warm-up failed (continuing):', e?.message || e); }
    this.asr.reset();
    console.log(`[nemotron] warm-up ${Math.round(performance.now() - tWarm)} ms`);

    this._loadMs = performance.now() - tLoad;
    this.onStatus?.('Nemotron ready — streaming transcription active (CPU/WASM, GPU free)', 100);
    return this;
  }

  /** Reset all streaming state for a fresh utterance/session. */
  reset() {
    this.asr?.reset();
    this._pending.length = 0;
    this._chain = Promise.resolve();
    this._chunkCount = 0; this._totalChunkMs = 0; this._lastChunkMs = 0;
    this._audioSecs = 0; this._startedAt = 0; this._firstTextAt = 0;
  }

  /** Feed 16 kHz mono Float32 samples. Buffers, then drains whole chunks single-file. */
  feed(samples) {
    if (!this.asr || !samples || !samples.length) return;
    if (!this._startedAt) this._startedAt = performance.now();
    for (let i = 0; i < samples.length; i++) this._pending.push(samples[i]);
    this._kick(false);
  }

  /** Drain the buffer + decode the trailing partial chunk. Call once at end of stream. */
  async finalize() {
    this._kick(true);
    await this._chain;                       // ensure all queued chunks have decoded
    if (!this.asr) return '';
    const tail = await this.asr.finalize();
    if (tail) { this._emit(tail); }
    return tail || '';
  }

  /** Latency snapshot for benchmarking / on-screen readout. */
  stats() {
    const avg = this._chunkCount ? this._totalChunkMs / this._chunkCount : 0;
    return {
      loadMs: Math.round(this._loadMs),
      chunks: this._chunkCount,
      avgChunkMs: Math.round(avg),
      lastChunkMs: Math.round(this._lastChunkMs),
      audioSecs: +this._audioSecs.toFixed(2),
      // RTF = processing-time / audio-duration. < 1.0 means faster than realtime.
      rtf: this._audioSecs ? +((this._totalChunkMs / 1000) / this._audioSecs).toFixed(3) : 0,
      // time from first audio to first visible text (the "lag" the user feels)
      timeToFirstTextMs: (this._firstTextAt && this._startedAt) ? Math.round(this._firstTextAt - this._startedAt) : 0,
      // samples waiting in the feed buffer — sustained growth means decode can't keep up
      pendingSamples: this._pending.length,
    };
  }

  // ── internals ──
  _kick(final) {
    this._chain = this._chain.then(() => this._drain(final)).catch((e) => {
      console.warn('[nemotron] chunk decode error', e && e.message || e);
    });
  }

  async _drain(final) {
    if (!this.asr) return;
    while (this._pending.length >= this.feedSamples || (final && this._pending.length > 0)) {
      const take = final ? this._pending.length : this.feedSamples;
      const c = this._pending.splice(0, take);
      const buf = Float32Array.from(c);
      const t0 = performance.now();
      const txt = await this.asr.transcribe_chunk(buf);
      const dt = performance.now() - t0;
      this._chunkCount++; this._totalChunkMs += dt; this._lastChunkMs = dt;
      this._audioSecs += buf.length / 16000;
      if (txt) this._emit(txt);
      if (final) break;   // final pass takes everything in one shot
    }
  }

  _emit(txt) {
    if (!this._firstTextAt) this._firstTextAt = performance.now();
    this.onText?.(txt);
  }

  async _fetchBytes(url, onProgress) {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`fetch ${url}: ${r.status}`);
    // Progress requires a readable body + a known length; fall back to a plain buffer otherwise.
    const len = +(r.headers.get('content-length') || 0);
    if (!onProgress || !r.body || !len) return new Uint8Array(await r.arrayBuffer());
    const reader = r.body.getReader();
    const out = new Uint8Array(len);
    let off = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      out.set(value, off);
      off += value.length;
      onProgress(Math.min(1, off / len));
    }
    return out;
  }
}

export default NemotronEngine;
