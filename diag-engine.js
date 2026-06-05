/**
 * diag-engine.js — Rust/WASM crash-diagnostics sampler (Appendix A rows 34 + 35).
 *
 * The thin ES-module wrapper that REPLACES (strangler-fig) the inline `Diag`
 * IIFE, `window.dumpDiag` / `window.clearDiag`, and the load-time prior-trail
 * banner in index.html. The whole sampler — the bounded `notetakerDiag`
 * localStorage ring, the `toFixed(1)` / `JSON.stringify` row normalization, the
 * counter state machine, the prior-trail strings — now lives in Rust
 * (`silent-core::diag` policy + `silent-web::diag::DiagLayer` subscriber),
 * surfaced as `WasmDiag` in the shared `silent_web.js` pkg. This loader drives
 * that policy: every counter hook and the ~3 s sampler tick is a `silent.diag`
 * `tracing` event the installed `DiagLayer` folds into the byte-pinned ring.
 *
 * # What moved to Rust (the policy) vs what stays here (the host)
 *
 *   Policy (Rust `WasmDiag` + `DiagLayer` + `silent_core::Diag`):
 *     - the loop counters (loopIter / recycle / ctxLen / genStepsTotal /
 *       inputTokens / lastStepMs / deviceLost) and the `nTokens||1` advance
 *     - the bounded ring (push then evict to 200 rows) + the EXACT row shape and
 *       key order, byte-identical to the shipping JS `JSON.stringify(rows)`
 *     - the `performance.memory` → `*MB` `toFixed(1)` normalization
 *     - the prior-trail headline + per-row summary lines (the banner copy)
 *     - the PerfMonitor `EngineStats` snapshot (row 35) on the same target
 *
 *   Host (this loader + index.html):
 *     - the ~3 s `setInterval` timer (the core has no clock) and the trail-start
 *       epoch, so each tick supplies a wall-clock `iso` + whole `elapsedSec`
 *     - the DOM `.transcript-item` / `.transcript-word` counts and the Voxtral
 *       ring `writeAbs` (the `silent.diag` schema's host-supplied sample fields)
 *     - `getUserMedia` / the engine loop call sites that fire the counter hooks
 *
 * The `performance.memory` heap read and `performance.now()` step clock are read
 * INSIDE the wasm surface (the genuinely browser-bound part) — the host only
 * passes the clock/DOM/ring values the GPU/heap reader cannot see.
 *
 * # The boundary is bigint-coerced
 *
 * The `WasmDiag` counter/sample params are `u64` → `bigint` on the JS side, so
 * every numeric value is coerced with `BigInt(Math.trunc(...))` at the call
 * boundary (a `number` would throw `TypeError: Cannot convert … to a BigInt`).
 *
 * Privacy: no audio, transcript text, or model data crosses this surface — only
 * the diagnostic counters, heap/DOM counts, and the (already-on-device)
 * localStorage trail.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

/**
 * Shared, cross-loader module-init promise for the wasm-pack pkg (see
 * session-engine.js for the full rationale): one `import()` + `default()` shared
 * across ALL engine loaders via `window.__silentWebModulePromises`, so a
 * concurrent boot-time init in a sibling loader cannot double-instantiate the
 * wasm binary and corrupt every allocated object.
 */
function _loadModule(pkgUrl) {
  const w = (typeof window !== 'undefined') ? window : globalThis;
  const cache = (w.__silentWebModulePromises ||= new Map());
  let p = cache.get(pkgUrl);
  if (!p) {
    p = (async () => {
      const mod = await import(pkgUrl);
      await mod.default();
      return mod;
    })();
    cache.set(pkgUrl, p);
  }
  return p;
}

/** Coerce a host `number` to a non-negative `bigint` for a `u64` wasm param. */
function u64(n) {
  const v = Math.trunc(Number(n) || 0);
  return BigInt(v < 0 ? 0 : v);
}

/** The `notetakerDiag` localStorage key (mirrors `silent_core::diag::DIAG_KEY`). */
export const DIAG_KEY = 'notetakerDiag';

/** The sampler cadence in ms (mirrors `silent_core::diag::DIAG_INTERVAL_MS`). */
export const DIAG_INTERVAL_MS = 3000;

export class DiagEngine {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]
   * @param {() => boolean} [opts.enabled]  gate the whole sampler (the JS `DIAG`
   *                                         flag); defaults to always on.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;
    this._enabledFn = typeof opts.enabled === 'function' ? opts.enabled : () => true;

    this._mod = null;
    this._diag = null;        // WasmDiag instance
    this._loadPromise = null;
    this.ready = false;

    this._timer = null;       // the ~3 s sampler interval (the host owns the clock)
    this._startMs = null;     // trail-start epoch for elapsedSec
    /**
     * The host's per-tick sample-context provider. index.html sets this so each
     * sample reads the live DOM counts + Voxtral ring cursor.
     * @type {() => { items:number, words:number, writeAbs:number|null }}
     */
    this.sampleContext = () => ({ items: 0, words: 0, writeAbs: null });
  }

  get enabled() { return !!this._enabledFn(); }

  /** Load the wasm module and install the global diag tracing subscriber. */
  load() {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;
    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this._diag = new this._mod.WasmDiag();   // installs the global subscriber once
      this.ready = true;
      console.log('[rust-diag] DiagEngine ready (silent-core sampler on tracing; window.dumpDiag() to inspect after a freeze).');
    })();
    return this._loadPromise;
  }

  // ── trail lifecycle (the JS Diag.start / Diag.stop) ──────────────────────────

  /**
   * Begin a fresh trail (called at Voxtral-session start): clear the prior trail,
   * start the ~3 s timer, and take the baseline sample at t=0. Mirrors the JS
   * `Diag.start()` (reset + setInterval + one baseline `sample()`).
   */
  start() {
    if (!this.enabled || !this._diag) return;
    this._diag.start();                 // clears the trail + zeroes the counters
    this._startMs = Date.now();
    if (this._timer) clearInterval(this._timer);
    this._timer = setInterval(() => this._tick(), DIAG_INTERVAL_MS);
    this._tick();                       // baseline row at t=0
    console.log('[DIAG] sampler started; window.dumpDiag() to inspect after a freeze.');
  }

  /**
   * Stop the timer and take one final post-stop sample (the JS `Diag.stop()`
   * recorded a final row to catch "keeps climbing after stop" drift).
   */
  stop() {
    if (this._timer) { clearInterval(this._timer); this._timer = null; }
    if (this.enabled && this._diag) {
      this._tick();                     // final post-stop row
      this._diag.stop();
      console.log('[DIAG] sampler stopped.');
    }
  }

  // ── loop-side hooks (cheap; fired from the engine loop) ──────────────────────

  /** A NEW generate() context is starting (records the prompt token count). */
  onLoopIter(inputTokens) {
    if (this.enabled && this._diag) this._diag.onLoopIter(u64(inputTokens));
  }

  /** A generate() return hit the recycle cap (the bounded sawtooth event). */
  onRecycle() {
    if (this.enabled && this._diag) this._diag.onRecycle();
  }

  /** One streamer put() happened (the wasm side reads performance.now() itself). */
  onPut(nTokens) {
    if (this.enabled && this._diag) this._diag.onPut(u64(nTokens));
  }

  /**
   * A WebGPU device-lost / OOM error was observed (which performance.memory
   * CANNOT see). Records the message AND takes an immediate out-of-band sample,
   * exactly as the JS `onDeviceLost` did — in one event. Supplies the same
   * host-clocked sample context the timer tick does.
   */
  onDeviceLost(msg) {
    if (!this.enabled || !this._diag) return;
    const ctx = this._readContext();
    this._diag.onDeviceLost(
      String(msg || 'device-lost'),
      ctx.iso,
      u64(ctx.elapsedSec),
      u64(ctx.items),
      u64(ctx.words),
      ctx.writeAbs,
    );
  }

  // ── PerfMonitor (row 35) ─────────────────────────────────────────────────────

  /**
   * Record an EngineStats snapshot on the same tracing target (telemetry, not a
   * crash sample — it does NOT write a diag row). Pass the object from
   * `nemotron.stats()` (rtf/avgChunkMs/lastChunkMs/pendingSamples/chunks).
   * @param {object} s
   */
  recordStats(s) {
    if (!this.enabled || !this._diag || !s) return;
    this._diag.recordStats(
      u64(s.loadMs || s.load_ms || 0),
      u64(s.chunks || 0),
      u64(s.avgChunkMs || s.avg_chunk_ms || 0),
      u64(s.lastChunkMs || s.last_chunk_ms || 0),
      Number(s.audioSecs || s.audio_secs || 0),
      Number(s.rtf || 0),
      u64(s.ttftMs || s.timeToFirstTextMs || s.ttft_ms || 0),
      u64(s.pendingSamples || s.pending_samples || 0),
    );
  }

  /** The latest EngineStats snapshot (parsed), or null. Polled by the PerfMonitor. */
  takeStats() {
    if (!this._diag) return null;
    try { return JSON.parse(this._diag.takeStats()); } catch (_) { return null; }
  }

  // ── retrieval (window.dumpDiag / clearDiag) + prior-trail banner ─────────────

  /** The stored trail rows (parsed array) — the same shape `dumpDiag()` returns. */
  rows() {
    if (!this._diag) {
      // Pre-load fallback: read the raw localStorage trail directly so a freeze
      // before the wasm finished loading is still inspectable.
      try { return JSON.parse(localStorage.getItem(DIAG_KEY) || '[]'); } catch (_) { return []; }
    }
    try { return JSON.parse(this._diag.rowsJson()); } catch (_) { return []; }
  }

  /** Clear the stored trail (the JS `window.clearDiag()`). */
  clear() {
    if (this._diag) { this._diag.clear(); return; }
    try { localStorage.removeItem(DIAG_KEY); } catch (_) {}
  }

  /**
   * The prior-trail banner `{ headline, summaryLines }` for the load-time
   * surfacing after a non-clean shutdown. Reads the stored trail and returns the
   * byte-pinned Rust strings (headline + last-5 summary lines, oldest first).
   * Empty trail ⇒ `{ headline: '', summaryLines: [] }`.
   */
  priorTrailBanner() {
    if (!this._diag) return { headline: '', summaryLines: [] };
    // Mirrors `takeStats()` above: the wasm binding returns a JSON STRING
    // (`prior_trail_banner` → `JsValue::from_str`), so it must be parsed before
    // the caller can read `.headline` / `.summaryLines`.
    try { return JSON.parse(this._diag.priorTrailBanner()); } catch (_) { return { headline: '', summaryLines: [] }; }
  }

  // ── internals ────────────────────────────────────────────────────────────────

  /** Assemble the per-tick host sample context (clock + DOM + ring cursor). */
  _readContext() {
    const now = Date.now();
    const elapsedSec = this._startMs ? Math.round((now - this._startMs) / 1000) : 0;
    let ctx = { items: 0, words: 0, writeAbs: null };
    try { ctx = this.sampleContext() || ctx; } catch (_) {}
    // `writeAbs` crosses as i64 with -1 as the "no ring" sentinel (→ null).
    const writeAbs = (ctx.writeAbs === null || ctx.writeAbs === undefined)
      ? -1n : BigInt(Math.trunc(Number(ctx.writeAbs)));
    return {
      iso: new Date(now).toISOString(),
      elapsedSec,
      items: ctx.items || 0,
      words: ctx.words || 0,
      writeAbs,
    };
  }

  /** One sampler tick: emit the `sample` event with the host-supplied context. */
  _tick() {
    if (!this.enabled || !this._diag) return;
    const ctx = this._readContext();
    this._diag.sample(ctx.iso, u64(ctx.elapsedSec), u64(ctx.items), u64(ctx.words), ctx.writeAbs);
  }
}

export default DiagEngine;
