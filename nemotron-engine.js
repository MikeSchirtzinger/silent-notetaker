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

const DEFAULT_MODEL_BASE = new URL('./nemotron-asr/models/', import.meta.url).href;
const DEFAULT_PKG_URL    = new URL('./nemotron-asr/pkg/nemotron_asr.js', import.meta.url).href;
// How much audio to buffer before each transcribe_chunk call. This is the dominant lever
// on *perceived* latency: the model's native chunk is ~560 ms, RTF is ~0.40 regardless, and
// accuracy stays word-for-word correct down to 250 ms feeds (measured). So perceived latency
// ≈ feedMs + ~0.4·feedMs. 500 ms keeps it snappy (~0.7 s) without over-fragmenting; bump to
// 250 ms for minimum lag, or 1000 ms to minimise call overhead. Override via opts.feedSamples.
const FEED_SAMPLES       = 8000;    // 500 ms @ 16 kHz

export class NemotronEngine {
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.modelBase   = opts.modelBase || w.__NEMOTRON_MODEL_BASE || DEFAULT_MODEL_BASE;
    this.pkgUrl      = opts.pkgUrl    || w.__NEMOTRON_PKG_URL    || DEFAULT_PKG_URL;
    this.feedSamples = opts.feedSamples || FEED_SAMPLES;

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
    this.asr = await this._WasmAsr.create(enc, dec, tok);
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
