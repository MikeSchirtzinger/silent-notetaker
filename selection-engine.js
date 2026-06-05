/**
 * selection-engine.js — Rust/WASM registry-driven engine-selection engine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes a `SelectionEngine` that `index.html`'s
 * settings model picker drives — the same strangler-fig pattern as
 * `session-engine.js`, `exports-engine.js`, `notes-engine.js`, and
 * `storage-engine.js`. It loads the SAME `silent_web.js` pkg module the other
 * engines load (one wasm binary, many surfaces) via the shared cross-loader
 * init promise.
 *
 * It wraps the `silent_inference::selection` policy (PRD Phase 5, Task I3;
 * Appendix A rows 7, 8):
 *
 *   - `asrPickerOptions(probe)`   — the settings ASR picker option list, sourced
 *     from the embedded registry's `ui` entries (every shipping engine incl.
 *     Nemotron, exactly as today), each with row-8 backend/precision data and an
 *     availability verdict for the probed device.
 *   - `resolveSelection(value, probe)` — resolve a persisted `settings.model`
 *     key to its option (availability included).
 *   - `recommendedDefault(probe)` — the per-tier recommended default engine the
 *     UI may show (user choice always wins; this never overrides it).
 *   - `resolveTier(probe)`        — the detected device tier string.
 *   - `applySelection(value, isRecording)` — the queued mid-recording switch:
 *     idle → applies now; recording → queued for next meeting with a friendly
 *     notice. Never a silent failure, never a hard reject.
 *
 * # The JS this REPLACES (strangler-fig)
 *
 * The hand-maintained `MODEL_OPTIONS` table in index.html (the picker list) is
 * now SOURCED from the registry: the picker maps over `asrPickerOptions()`
 * instead of the JS literal. The device-tier mapping that `GpuCaps`/`resolveTier`
 * compute moves into Rust (`resolve_tier`); the JS `GpuCaps.probe()` stays only
 * as the raw `navigator` reader that fills the typed `DeviceProbe`.
 *
 * # The DeviceProbe
 *
 * `probe()` builds the typed `DeviceProbe` the Rust policy consumes from the same
 * `navigator` fields the shipping `GpuCaps.probe()` reads: `navigator.gpu`
 * adapter presence + `isFallbackAdapter`, the adapter's `limits.maxBufferSize`,
 * `navigator.deviceMemory`, `navigator.hardwareConcurrency`, and
 * `crossOriginIsolated`. It is async because `requestAdapter()` is async; the
 * result is cached after the first probe (the device does not change mid-session),
 * exactly as `GpuCaps` caches its `probed` flag.
 *
 * Privacy: no audio or model data crosses this surface — only the device
 * capabilities and the selection decisions. The registry is embedded in the wasm
 * binary, so this surface performs NO network I/O and adds no CSP egress.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

/**
 * Shared, cross-loader module-init promise for the wasm-pack pkg (see
 * session-engine.js for the full rationale): one `import()` + `default()` across
 * ALL engine loaders, keyed by pkg URL, so a concurrent boot-time init never
 * double-initializes the wasm binary and corrupts the heap.
 */
function _loadModule(pkgUrl) {
  const w = (typeof window !== 'undefined') ? window : globalThis;
  const cache = (w.__silentWebModulePromises ||= new Map());
  let p = cache.get(pkgUrl);
  if (!p) {
    p = (async () => {
      const mod = await import(pkgUrl);
      await mod.default();   // initialises the wasm binary exactly once
      return mod;
    })();
    cache.set(pkgUrl, p);
  }
  return p;
}

/**
 * Read the raw device capabilities into a typed `DeviceProbe` the Rust policy
 * consumes. Mirrors the shipping `GpuCaps.probe()` reads exactly:
 *   - a real, non-fallback WebGPU adapter → `webgpu_available: true`
 *   - the adapter's `limits.maxBufferSize` → `max_gpu_buffer_gb`
 *   - `navigator.deviceMemory` (Chrome caps at 8) → `memory_gb` (null if absent)
 *   - `navigator.hardwareConcurrency || 4` → `thread_count`
 *   - `crossOriginIsolated` → `cross_origin_isolated`
 *
 * @returns {Promise<object>} the DeviceProbe (snake_case keys for serde).
 */
async function buildDeviceProbe() {
  const cores = (typeof navigator !== 'undefined' && navigator.hardwareConcurrency) || 4;
  const memGB = (typeof navigator !== 'undefined' && navigator.deviceMemory) || null;
  const coi = (typeof crossOriginIsolated !== 'undefined') ? !!crossOriginIsolated : false;

  let webgpuAvailable = false;
  let maxBufGB = 0;
  try {
    const adapter = (typeof navigator !== 'undefined' && navigator.gpu)
      ? await navigator.gpu.requestAdapter()
      : null;
    if (adapter && !adapter.isFallbackAdapter) {
      webgpuAvailable = true;
      const maxBuf = (adapter.limits && adapter.limits.maxBufferSize) || 0;
      maxBufGB = maxBuf / (1024 ** 3);
    }
    // A software/fallback adapter is treated as CPU (webgpuAvailable stays false),
    // matching GpuCaps ("software WebGPU only — treating as CPU").
  } catch (_) {
    // Probe failure → CPU-only, matching GpuCaps' catch branch.
    webgpuAvailable = false;
  }

  return {
    webgpu_available: webgpuAvailable,
    memory_gb: memGB,
    cross_origin_isolated: coi,
    thread_count: cores,
    max_gpu_buffer_gb: maxBufGB,
  };
}

export class SelectionEngine {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]  Override for the wasm-pack pkg URL.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;

    this._mod = null;
    this._loadPromise = null;
    this._probe = null;        // cached DeviceProbe (device does not change mid-session)
    this.ready = false;
  }

  /**
   * Load the wasm module. Idempotent — safe to call multiple times (returns the
   * same promise after the first call).
   */
  load() {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;

    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this.ready = true;
      console.log('[rust-selection] SelectionEngine ready (registry-driven picker + device tiers + queued switch)');
    })();

    return this._loadPromise;
  }

  /** @returns {object} the loaded wasm module (throws if not loaded). */
  _m() {
    if (!this._mod) throw new Error('[rust-selection] SelectionEngine not loaded — call load() first');
    return this._mod;
  }

  /**
   * The cached typed device probe (built once from `navigator`). Pass
   * `force = true` to re-probe.
   * @param {boolean} [force]
   * @returns {Promise<object>}
   */
  async probe(force = false) {
    if (!this._probe || force) this._probe = await buildDeviceProbe();
    return this._probe;
  }

  /**
   * The settings ASR picker option list, registry-driven, with per-engine
   * availability for the probed device (Appendix A rows 7, 8).
   * @returns {Promise<Array<{value:string,label:string,model_id:string,backend:string,precision:string[],availability:object}>>}
   */
  async asrPickerOptions() {
    await this.load();
    const probe = await this.probe();
    return JSON.parse(this._m().asrPickerOptions(JSON.stringify(probe)));
  }

  /**
   * Resolve a persisted picker `value` (a stored `settings.model`) to its option.
   * @param {string} value
   * @returns {Promise<object>}
   */
  async resolveSelection(value) {
    await this.load();
    const probe = await this.probe();
    return JSON.parse(this._m().resolveSelection(String(value || ''), JSON.stringify(probe)));
  }

  /**
   * The per-tier recommended default ASR engine for this device (the UI may show
   * it; user choice always wins).
   * @returns {Promise<string|null>}
   */
  async recommendedDefault() {
    await this.load();
    const probe = await this.probe();
    return JSON.parse(this._m().recommendedDefault(JSON.stringify(probe)));
  }

  /**
   * The detected device tier string (`wasm_only`/`webgpu_low`/`webgpu_mid`/`webgpu_high`).
   * @returns {Promise<string>}
   */
  async resolveTier() {
    await this.load();
    const probe = await this.probe();
    return JSON.parse(this._m().resolveTier(JSON.stringify(probe)));
  }

  /**
   * The queued mid-recording switch policy. Idle → applies now; recording →
   * queued for the next meeting with a friendly notice. Synchronous policy
   * (no device probe needed) but async to keep the load() guarantee.
   * @param {string} value         the chosen picker value
   * @param {boolean} isRecording  whether a recording is in progress
   * @returns {Promise<{outcome:string,value:string,notice?:string}>}
   */
  async applySelection(value, isRecording) {
    await this.load();
    return JSON.parse(this._m().applySelection(String(value || ''), !!isRecording));
  }
}
