/**
 * bridge-engine.js — Rust/WASM Claude-bridge reconnect/backoff policy engine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes a `BridgeReconnect` the inline
 * `ClaudeBridge` WebSocket client drives — the same strangler-fig pattern as
 * `session-engine.js` / `storage-engine.js` / `notes-engine.js`. It loads the
 * SAME `silent_web.js` pkg module the other engines load (one wasm binary, many
 * surfaces) via the shared cross-loader init promise.
 *
 * It wraps the `silent-core::bridge::ReconnectPolicy` (PRD Phase 4, Appendix A
 * row 28): the deterministic connection-status + exponential-backoff state
 * machine. Per the PRD the WebSocket itself stays in JS — `index.html`'s
 * `ClaudeBridge` keeps `new WebSocket()`, `onopen`/`onclose`/`onmessage`, and the
 * `setTimeout` that arms the next attempt. What moved to Rust is the *policy*:
 *
 *   - whether to auto-reconnect (only after a prior successful connection — the
 *     old `if (wasConnected)` guard, now a typed rule),
 *   - after how long (the `5s → 10s → 20s → 40s`, capped `60s` schedule that
 *     replaced the flat `setTimeout(…, 5000)`),
 *   - and what status the dot/label show.
 *
 * The executor reports lifecycle facts (connectRequested / open / closed /
 * manualConnect) and acts on the returned action:
 *   { action: 'connect' }                  → open a socket now
 *   { action: 'schedule_reconnect', delay_ms } → arm setTimeout(delay_ms), then
 *                                                 call connectRequested() again
 *   { action: 'none' }                     → do nothing
 *
 * Privacy: this module carries connection STATE only — never transcript text,
 * screenshots, audio, or any meeting content. Those flow over the socket the JS
 * executor owns; the policy only decides whether the socket should exist.
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

export class BridgeReconnect {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]  Override for the wasm-pack pkg URL.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;

    this._mod = null;
    this._policy = null;        // WasmBridgeReconnect instance
    this._loadPromise = null;
    this.ready = false;
  }

  /**
   * Load the wasm module and construct the policy object. Idempotent — safe to
   * call multiple times (returns the same promise after the first call).
   */
  load() {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;

    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this._policy = new this._mod.WasmBridgeReconnect();
      this.ready = true;
      console.log('[rust-bridge] BridgeReconnect ready (reconnect/backoff policy)');
    })();

    return this._loadPromise;
  }

  /**
   * A connection attempt is being made now (the executor's `connect()` body or
   * an armed reconnect timer firing).
   * @returns {{action:'connect'}|{action:'none'}}
   */
  connectRequested() {
    if (!this._policy) return { action: 'none' };
    return JSON.parse(this._policy.connectRequested());
  }

  /**
   * A manual connect from the setup panel's "Connect" button. Resets the
   * backoff and always opens (regardless of prior success).
   * @returns {{action:'connect'}|{action:'none'}}
   */
  manualConnect() {
    if (!this._policy) return { action: 'none' };
    return JSON.parse(this._policy.manualConnect());
  }

  /**
   * The socket's `onopen` fired — clears backoff, records success, lights the
   * dot.
   * @returns {{action:'none'}}
   */
  open() {
    if (!this._policy) return { action: 'none' };
    return JSON.parse(this._policy.open());
  }

  /**
   * The socket's `onclose` fired.
   * @returns {{action:'schedule_reconnect', delay_ms:number}|{action:'none'}}
   */
  closed() {
    if (!this._policy) return { action: 'none' };
    return JSON.parse(this._policy.closed());
  }

  /**
   * Apply the user's settings toggle (`settings.claudeBridge`). A disabled
   * bridge does not connect or auto-reconnect.
   * @param {boolean} enabled
   */
  setEnabled(enabled) {
    if (this._policy) this._policy.setEnabled(!!enabled);
  }

  /** Reset to the fresh, never-connected state (manual disconnect / teardown). */
  reset() {
    if (this._policy) this._policy.reset();
  }

  /** @returns {boolean} whether the dot should be lit (status === connected). */
  isConnected() {
    return this._policy ? this._policy.isConnected() : false;
  }

  /** @returns {string} status key: disconnected/connecting/connected/reconnecting. */
  status() {
    return this._policy ? this._policy.status() : 'disconnected';
  }

  /** @returns {number} consecutive-reconnect attempt counter (witness/diagnostics). */
  reconnectAttempt() {
    return this._policy ? this._policy.reconnectAttempt() : 0;
  }
}
