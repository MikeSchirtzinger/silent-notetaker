/**
 * notes-engine.js — Rust/WASM notes + smart-questions + Qwen policy engine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes a `NotesEngine` that the `index.html`
 * notes path imports and drives — the same strangler-fig pattern as
 * `diarization-engine.js` does for diarization and `nemotron-engine.js` does
 * for the ASR engine. It loads the SAME `silent_web.js` pkg module the
 * diarization engine loads (one wasm binary, two surfaces).
 *
 * The engine wraps three Rust policy surfaces (PRD Phase 3, Appendix A rows
 * 16, 18, 19, 21, 22, 35-partial):
 *
 *   - `WasmNoteEngine`        — the live trigger extractor + open-question
 *                               tracker (replaces the JS `NoteEngine` class
 *                               and the `OpenQs` object).
 *   - `WasmQuestionScheduler` — the SmartQ teleprompter scheduler (replaces the
 *                               JS `SmartQ` policy: timing/char gates, type
 *                               rotation, dedup ring, reroll/minimize/badge).
 *   - Qwen free functions     — `parseQwenNotes` / `chunkTranscript` /
 *                               `dedupeNotes` / `finalNotesChunks` /
 *                               `recapCleanGroup` (replace the JS functions of
 *                               the same name).
 *
 * The `question-worker.js` Qwen worker stays the EXECUTOR. The Rust scheduler
 * emits typed `GenerateRequest`s the caller forwards to the worker; the worker
 * reply is routed back via `scheduler.onWorkerResult(requestId, text)`. The
 * NOTES_SYSTEM prompt, the per-type recap system prompts, and all DOM rendering
 * stay in `index.html` — only the POLICY moved.
 *
 * Privacy: no transcript text leaves the browser through this module. The
 * policy runs entirely inside the Rust WASM heap; only categorized note text,
 * question text, and generation requests (which the local worker consumes)
 * cross the boundary — exactly as before.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

/**
 * Singleton module handle for the wasm-pack pkg. Loading is idempotent and
 * shared with `diarization-engine.js` (both import the same module URL, so the
 * browser module cache returns the same instance; `mod.default()` is a no-op
 * after the first init). We still guard the init so a `NotesEngine` created
 * before the diarization engine initializes the wasm binary works standalone.
 */
//
// Shared, cross-loader module-init promise (see session-engine.js for the full
// rationale): `silent_web.js`'s `default()` init is only idempotent AFTER the
// first init resolves, so two loaders calling it concurrently (the boot race
// between this notes loader and session-engine.js) double-initialize the wasm
// binary and corrupt the heap. One promise per pkg URL, shared across every
// engine loader via the `window.__silentWebModulePromises` map, guarantees a
// single `import()` + `default()`.
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

export class NotesEngine {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]  Override for the wasm-pack pkg URL.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;

    this._mod = null;
    this._note = null;          // WasmNoteEngine instance
    this._scheduler = null;     // WasmQuestionScheduler instance
    this._corrections = null;   // WasmCorrections instance
    this._loadPromise = null;
    this.ready = false;
  }

  /**
   * Load the wasm module and construct the policy objects. Idempotent — safe to
   * call multiple times (returns the same promise after the first call).
   *
   * @param {string[]} [enabledTypes]  The `settings.smartqTypes` subset that
   *   rotates in the scheduler (e.g. `['clarify','risk',...]`). Empty / omitted
   *   uses the full default rotation.
   */
  load(enabledTypes) {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;

    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this._note = new this._mod.WasmNoteEngine();
      this._scheduler = new this._mod.WasmQuestionScheduler(
        Array.isArray(enabledTypes) && enabledTypes.length ? JSON.stringify(enabledTypes) : null
      );
      this._corrections = new this._mod.WasmCorrections();
      this.ready = true;
      console.log('[rust-notes] NotesEngine ready (NoteExtractor + OpenQs + SmartQ scheduler + Corrections + Qwen)');
    })();

    return this._loadPromise;
  }

  /**
   * Rebuild the smart-question scheduler with a new enabled-types subset
   * (when settings change). Preserves the note engine. Resets scheduling state
   * — matching the JS, where `_enabledTypes()` is read fresh on each generate.
   *
   * @param {string[]} enabledTypes
   */
  setEnabledQuestionTypes(enabledTypes) {
    if (!this._mod) return;
    this._scheduler = new this._mod.WasmQuestionScheduler(
      Array.isArray(enabledTypes) && enabledTypes.length ? JSON.stringify(enabledTypes) : null
    );
  }

  // ── Live trigger notes (WasmNoteEngine) ────────────────────────────────────

  /**
   * Categorize a final transcript line (`NoteEngine.analyze`).
   * @param {string} text
   * @returns {Array<{category:string, text:string, triggerPhrase:string}>}
   */
  analyze(text) {
    if (!this._note) return [];
    return JSON.parse(this._note.analyze(text));
  }

  /**
   * Consider a transcript line as a potential answer to open questions
   * (`OpenQs.consider`). Call BEFORE the add loop. Skips question lines.
   * @param {string} text
   * @returns {number[]}  Note ids (JS numbers) newly resolved by this line.
   */
  consider(text) {
    if (!this._note) return [];
    return JSON.parse(this._note.consider(text));
  }

  /**
   * Register a newly-detected open question (`OpenQs.add(id, text)`). Only call
   * for notes whose category is `questions`, AFTER `db.notes.add` assigns `id`.
   * Never called for stop-flush results (parity with index.html).
   * @param {number} id    The db.notes id (a JS number).
   * @param {string} text
   */
  addQuestion(id, text) {
    if (!this._note) return;
    this._note.add_question(Number(id), text);
  }

  /**
   * Force-flush the trailing sentence buffer at stop (`NoteEngine.flush`).
   * @returns {Array<{category:string, text:string, triggerPhrase:string}>}
   */
  flush() {
    if (!this._note) return [];
    return JSON.parse(this._note.flush());
  }

  /**
   * The number of still-open (unresolved) questions (`OpenQs` open count).
   * @returns {number}
   */
  openCount() {
    return this._note ? this._note.openCount() : 0;
  }

  /**
   * The texts of the still-open questions, in insertion order
   * (`OpenQs.openTexts`). Used by the stop-time question recap.
   * @returns {string[]}
   */
  openTexts() {
    if (!this._note) return [];
    return JSON.parse(this._note.openTexts());
  }

  /**
   * Reset the extractor buffer + open-question tracker for a new meeting
   * (`new NoteEngine()` + `OpenQs.reset()`).
   */
  resetNotes() {
    if (this._note) this._note.reset();
  }

  // ── Word corrections (WasmCorrections) ──────────────────────────────────────
  //
  // The insertion-ordered { wrong: right } map + the case-insensitive, global,
  // sequential apply that replaced the JS `corrections` object +
  // `applyCorrections` / `applyCorrectionsToTranscript` regex policy.

  /**
   * Add (or overwrite) a correction (`addCorrection`:
   * `corrections[wrong] = right`). Empty wrong/right are ignored.
   * @param {string} wrong
   * @param {string} right
   * @returns {boolean} whether the map changed.
   */
  addCorrection(wrong, right) {
    return this._corrections ? this._corrections.add(wrong, right) : false;
  }

  /**
   * Remove a correction by its `wrong` key (`removeCorrection`:
   * `delete corrections[wrong]`).
   * @param {string} wrong
   * @returns {boolean} whether a pair was removed.
   */
  removeCorrection(wrong) {
    return this._corrections ? this._corrections.remove(wrong) : false;
  }

  /**
   * Replace the whole correction map from an ordered array of pairs (restore).
   * @param {Array<{wrong:string, right:string}>} corrections
   */
  setCorrections(corrections) {
    if (this._corrections) this._corrections.set(JSON.stringify(corrections || []));
  }

  /**
   * The current correction map as an ordered array of `{ wrong, right }` pairs
   * (insertion order). Used to re-render the tags + re-push to the worker.
   * @returns {Array<{wrong:string, right:string}>}
   */
  correctionEntries() {
    return this._corrections ? JSON.parse(this._corrections.entries()) : [];
  }

  /**
   * Apply every correction to `text` (`applyCorrections`): case-insensitive,
   * global, in insertion order. The single live + retroactive policy.
   * @param {string} text
   * @returns {string}
   */
  applyCorrections(text) {
    return this._corrections ? this._corrections.apply(text) : text;
  }

  /** Clear all corrections (new meeting). */
  clearCorrections() {
    if (this._corrections) this._corrections.clear();
  }

  // ── Smart-question scheduler (WasmQuestionScheduler) ────────────────────────
  //
  // The scheduler externalizes the JS two-attempt dedup loop. accumulate/reroll
  // return either { kind:'accumulated' } or { kind:'generate', request, expanded }.
  // The caller runs the worker on `request` (a GenerateRequest payload carrying
  // request_id, window, kind) and routes the reply back via onWorkerResult.

  /**
   * Accumulate a finalized transcript fragment (`SmartQ.accumulate`).
   * @param {string} text
   * @param {number} nowMs  Date.now() (the scheduler clock).
   * @returns {{kind:'accumulated'}|{kind:'generate', request:object, expanded:boolean}}
   */
  accumulate(text, nowMs) {
    if (!this._scheduler) return { kind: 'accumulated' };
    return JSON.parse(this._scheduler.accumulate(text, nowMs));
  }

  /**
   * Force a fresh generation (`SmartQ.reroll` / `rerollSmartQ`). `expanded` is
   * true when a minimized bar was expanded (the caller then clears the badge).
   * @param {number} nowMs
   * @returns {{kind:'accumulated'}|{kind:'generate', request:object, expanded:boolean}}
   */
  reroll(nowMs) {
    if (!this._scheduler) return { kind: 'accumulated' };
    return JSON.parse(this._scheduler.reroll(nowMs));
  }

  /**
   * Route a worker reply back into the scheduler (`SmartQ._generate` loop body).
   * @param {number} requestId  The `request_id` from the GenerateRequest.
   * @param {string} text       The worker's raw question text.
   * @returns {{kind:'ready', event:object}|{kind:'retry', request:object}|{kind:'superseded'}}
   *   ready → render `event` (a QuestionReady payload with text/kind/badge);
   *   retry → forward `request` to the worker again; superseded → drop.
   */
  onWorkerResult(requestId, text) {
    if (!this._scheduler) return { kind: 'superseded' };
    return JSON.parse(this._scheduler.onWorkerResult(requestId, text));
  }

  /**
   * Toggle the teleprompter minimize/expand state (`toggleSmartQ`).
   * @returns {{minimized:boolean}}  The new MinimizeChanged payload.
   */
  toggleMinimize() {
    if (!this._scheduler) return { minimized: true };
    return JSON.parse(this._scheduler.toggleMinimize());
  }

  /** Reset all scheduling state for a new meeting (`SmartQ.reset`). */
  resetScheduler() {
    if (this._scheduler) this._scheduler.reset();
  }

  /** @returns {boolean} whether the bar is minimized. */
  isMinimized() {
    return this._scheduler ? this._scheduler.isMinimized() : true;
  }

  /** @returns {boolean} whether the new-question badge dot is raised. */
  hasNewBadge() {
    return this._scheduler ? this._scheduler.hasNewBadge() : false;
  }

  // ── Qwen final-notes pipeline (free functions) ──────────────────────────────

  /**
   * Parse Qwen note output (`parseQwenNotes`).
   * @param {string} raw
   * @returns {Array<{cat:string, text:string, topic:(string|null)}>}
   */
  parseQwenNotes(raw) {
    return this._mod ? JSON.parse(this._mod.parseQwenNotes(raw)) : [];
  }

  /**
   * Split a transcript into ~target-char chunks (`chunkTranscript`).
   * @param {string} text
   * @param {number} target
   * @returns {string[]}
   */
  chunkTranscript(text, target) {
    return this._mod ? JSON.parse(this._mod.chunkTranscript(text, target >>> 0)) : [];
  }

  /**
   * Drop near-duplicate notes (`dedupeNotes`).
   * @param {Array<{cat:string, text:string, topic:(string|null)}>} notes
   * @returns {Array<{cat:string, text:string, topic:(string|null)}>}
   */
  dedupeNotes(notes) {
    return this._mod ? JSON.parse(this._mod.dedupeNotes(JSON.stringify(notes))) : notes;
  }

  /**
   * The final-notes chunk list `generateFinalNotes` dispatches to the worker
   * (`target = max(500, ceil(len/18))`, capped at 22 chunks).
   * @param {string} transcript
   * @returns {string[]}
   */
  finalNotesChunks(transcript) {
    return this._mod ? JSON.parse(this._mod.finalNotesChunks(transcript)) : [];
  }

  /**
   * Clean one per-type recap group's raw model output (`generateQuestionRecap`
   * line cleaning): strip numbering/bullets/one edge quote, drop ≤6-char lines,
   * dedup, keep the first 3.
   * @param {string} raw
   * @returns {string[]}
   */
  recapCleanGroup(raw) {
    return this._mod ? JSON.parse(this._mod.recapCleanGroup(raw)) : [];
  }
}
