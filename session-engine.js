/**
 * session-engine.js — Rust/WASM recording-session state machine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes a `SessionEngine` that `index.html`'s
 * `App.start/stop/newMeeting/tickTimer/updateTabUI` drive — the same
 * strangler-fig pattern as `diarization-engine.js`, `notes-engine.js`, and
 * `nemotron-engine.js`. It loads the SAME `silent_web.js` pkg module the other
 * engines load (one wasm binary, many surfaces).
 *
 * It wraps one Rust policy surface (PRD Phase 4, Appendix A rows 1, 2, 3, 6, 31):
 *
 *   - `WasmSession` — the deterministic `silent_core::session::SessionMachine`.
 *     It owns the *law*: which transition is legal, cold-vs-warm start (the
 *     resume-WITHOUT-reload guarantee, row 2), the Mic/Tab source set (row 6),
 *     the 120-char auto-title (row 3), the timer projection (row 1), and which
 *     stop-time passes fire (recluster / final notes / question recap / auto
 *     summary — rows 15/19/21/31).
 *
 * The host (index.html) keeps the *hands*: it executes the side effects the
 * machine returns — load/resume the engine, open `getUserMedia`, run the
 * stop-time passes, open the summary modal — and renders the events (button
 * visibility, the recording dot, the timer, the source badges, the title input).
 *
 * # Wire format
 *
 * Each command method returns an `Outcome = { events, effects }` where every
 * `event` and `effect` is a `{ tag, payload }` object (the `SessionEvent` /
 * `SideEffect` serde tagging). The host iterates `events` then `effects` in
 * order. The convenience `apply*` methods below `JSON.parse` the wasm return for
 * the caller.
 *
 * # The state flags this REPLACES (strangler-fig)
 *
 * The JS `App.startTime` / `App.timerInterval` / `_lastDuration` / the
 * `canResume` ad-hoc derivation / the manual Mic/Tab badge toggling / the
 * `cycleTimeFormat` localStorage mutation are now DECIDED by the Rust machine.
 * index.html keeps `timerInterval` only as the host's 1 s tick handle (it asks
 * the machine for the string); the timer *values* (`startTime`, `_lastDuration`)
 * are owned by `WasmSession`.
 *
 * Privacy: no audio, transcript, or model data crosses this surface — only the
 * session decisions (state, sources, title, timer strings, stop-hook flags).
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

/**
 * Shared, cross-loader module-init promise for the wasm-pack pkg.
 *
 * CRITICAL: `silent_web.js`'s generated `default()` init is only idempotent
 * *after* the first init resolves (its guard is `if (wasm !== undefined)
 * return wasm`, and `wasm` stays `undefined` until init finishes). If two
 * loaders call `default()` CONCURRENTLY — as the App constructor does at boot,
 * where `new NoteEngine()` kicks off `notes-engine.js`'s load at the same time
 * this `SessionEngine` loads — both proceed past the guard and instantiate the
 * module twice. The second init resets the cached memory views and re-runs
 * `__wbindgen_start`, corrupting every object the first init already allocated
 * (witnessed as a "memory access out of bounds" deallocating a `NoteExtractor`
 * and "recursive use … unsafe aliasing" on the session machine).
 *
 * Each engine JS file previously kept its OWN module promise, so the per-file
 * guard could not see a concurrent init in a sibling file. The fix is one
 * promise shared across ALL loaders (`notes-engine.js` and `diarization-engine.js`
 * adopt the same global): `import()` + `default()` run exactly once, and every
 * other loader awaits that same promise. Keyed by pkg URL so an explicit
 * override still gets its own instance.
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

export class SessionEngine {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]  Override for the wasm-pack pkg URL.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;

    this._mod = null;
    this._session = null;       // WasmSession instance
    this._loadPromise = null;
    this.ready = false;
  }

  /**
   * Load the wasm module and construct the session machine. Idempotent — safe
   * to call multiple times (returns the same promise after the first call).
   */
  load() {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;

    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this._session = new this._mod.WasmSession();
      this.ready = true;
      console.log('[rust-session] SessionEngine ready (SessionMachine: start/stop/continue/new-meeting + sources + timer + stop-hooks)');
    })();

    return this._loadPromise;
  }

  /**
   * Push the host's `loadSettings()` booleans into the machine so the stop-time
   * hooks are computed from the user's settings (Appendix A rows 18–21, 31).
   * `notesModelOff` is the transcript-only mode flag (`qwenModel === 'off'`,
   * row 20) and is authoritative over the two model-driven passes. Call on load
   * and before each Stop (the JS reads settings fresh inside `stop()`).
   *
   * @param {object} cfg
   * @param {boolean} cfg.aiFinalNotes
   * @param {boolean} cfg.smartQuestions
   * @param {boolean} cfg.smartqRecap
   * @param {boolean} cfg.autoSummary
   * @param {boolean} cfg.notesModelOff
   */
  setConfig({ aiFinalNotes, smartQuestions, smartqRecap, autoSummary, notesModelOff }) {
    if (!this._session) return;
    this._session.setConfig(
      aiFinalNotes !== false,
      smartQuestions !== false,
      smartqRecap !== false,
      !!autoSummary,
      !!notesModelOff,
    );
  }

  // ── Commands (each returns the parsed { events, effects } outcome) ──────────

  /**
   * Start (or Continue) a recording (`App.start`). The machine decides
   * cold-vs-warm — the host reads the side effect (`load_engine_and_capture` vs
   * `resume_capture_no_reload`). `title` is the live `#meetingTitle` value.
   * @param {string} title
   * @param {number} nowMs  Date.now()
   * @returns {{events:object[], effects:object[]}}
   */
  start(title, nowMs) {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.start(title || '', nowMs));
  }

  /**
   * Explicit warm restart (`ResumeRecording`) — never re-resolves the title.
   * @param {number} nowMs
   * @returns {{events:object[], effects:object[]}}
   */
  resume(nowMs) {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.resume(nowMs));
  }

  /**
   * Stop the active recording (`App.stop`). Returns the stop-time hooks inside a
   * `run_stop_hooks` effect + a `stop_hooks` event.
   * @param {number} nowMs
   * @returns {{events:object[], effects:object[]}}
   */
  stop(nowMs) {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.stop(nowMs));
  }

  /**
   * Reset to a fresh meeting (`App.newMeeting`). Emits `title_changed`
   * ("Untitled Meeting"); the host then computes the locale auto-title and
   * installs it via {@link setTitle} (Appendix A row 3).
   * @returns {{events:object[], effects:object[]}}
   */
  newMeeting() {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.newMeeting());
  }

  /**
   * Set/replace the pending title; clamps to 120 chars and echoes the clamped
   * value back via `title_changed`.
   * @param {string} title
   * @returns {{events:object[], effects:object[]}}
   */
  setTitle(title) {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.setTitle(title || ''));
  }

  /**
   * Add tab/system audio (`App.shareTab` on-path, row 6). Only valid while
   * recording. On success: an `add_tab_audio` effect + a `sources_changed`
   * event raising the Tab badge.
   * @param {number} nowMs
   * @returns {{events:object[], effects:object[]}}
   */
  addTabAudio(nowMs) {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.addTabAudio(nowMs));
  }

  /**
   * Remove tab/system audio (`App.shareTab` off-path, or the stream ended).
   * @param {number} nowMs
   * @returns {{events:object[], effects:object[]}}
   */
  removeTabAudio(nowMs) {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.removeTabAudio(nowMs));
  }

  /**
   * Cycle the timestamp display mode `elapsed → clock → ago` (`cycleTimeFormat`,
   * row 24). Emits `timestamp_mode_changed`.
   * @returns {{events:object[], effects:object[]}}
   */
  cycleTimestampMode() {
    if (!this._session) return { events: [], effects: [] };
    return JSON.parse(this._session.cycleTimestampMode());
  }

  // ── Timer projection (the host calls these each 1 s tick) ───────────────────

  /**
   * The header timer string (`App.tickTimer`). Pass the host locale wall-clock
   * string as `clock` in clock mode; `null` otherwise.
   * @param {number} nowMs
   * @param {string|null} [clock]
   * @returns {string}
   */
  timerText(nowMs, clock) {
    return this._session ? this._session.timerText(nowMs, clock ?? undefined) : '00:00';
  }

  /**
   * The export duration string (`App.currentDurationStr`) — always mm:ss.
   * @param {number} nowMs
   * @returns {string}
   */
  currentDurationStr(nowMs) {
    return this._session ? this._session.currentDurationStr(nowMs) : '00:00';
  }

  /**
   * Format a per-line timestamp (`App.formatStamp`). Pass the host locale string
   * as `clock` in clock mode.
   * @param {number} tsMs
   * @param {number} nowMs
   * @param {string|null} [clock]
   * @returns {string}
   */
  formatStamp(tsMs, nowMs, clock) {
    return this._session ? this._session.formatStamp(tsMs, nowMs, clock ?? undefined) : '00:00';
  }

  // ── Read-only state accessors ───────────────────────────────────────────────

  /** @returns {string} the active (clamped) title. */
  get title() { return this._session ? this._session.title() : ''; }

  /** @returns {string} "elapsed"|"clock"|"ago". */
  get timestampMode() { return this._session ? this._session.timestampMode() : 'elapsed'; }

  /** @returns {string} "idle"|"loading"|"recording"|"stopped". */
  get state() { return this._session ? this._session.state() : 'idle'; }

  /** @returns {boolean} whether an engine is loaded (next start is warm — row 2). */
  get engineLoaded() { return this._session ? this._session.engineLoaded() : false; }

  /** @returns {boolean} whether mic capture is active. */
  get micActive() { return this._session ? this._session.micActive() : false; }

  /** @returns {boolean} whether tab/system audio is mixed in. */
  get tabActive() { return this._session ? this._session.tabActive() : false; }
}
