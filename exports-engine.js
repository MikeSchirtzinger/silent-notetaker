/**
 * exports-engine.js — Rust/WASM export + history-duration formatting engine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes an `ExportsEngine` that `index.html`'s
 * copy/export/history paths drive — the same strangler-fig pattern as
 * `session-engine.js`, `storage-engine.js`, `notes-engine.js`, and
 * `diarization-engine.js`. It loads the SAME `silent_web.js` pkg module the
 * other engines load (one wasm binary, many surfaces) via the shared
 * cross-loader init promise.
 *
 * It wraps the `silent-core` export + timestamp formatters (PRD Phase 4,
 * Appendix A rows 24, 30) as pure free functions:
 *
 *   - `notesToMarkdown`        — structured notes as Markdown (timestamp-aware).
 *   - `historyReplayMarkdown`  — the history-detail replay export (no stamps).
 *   - `executiveLine`          — the meeting-summary executive line (sing/plural).
 *   - `transcriptText`         — `copyTranscript`'s timestamp-aware join.
 *   - `summaryMarkdownWithAi`  — `copySummaryMarkdown`'s additive AI-notes append.
 *   - `formatDuration`         — the history-list `Nm Ns` duration string.
 *
 * The DOM still SUPPLIES the inputs (the visible note/transcript text + the
 * already-formatted per-line stamps the active timestamp mode produced, plus the
 * locale date/duration strings the browser computes). Only the FORMATTING policy
 * — section ordering, the empty-text filter, the `- [ts] text` shape, the
 * executive line, the AI append, the duration shape — moved into Rust, where it
 * is byte-identically golden-tested.
 *
 * # Wire format
 *
 * The DTO inputs (note/transcript-line/AI-group arrays) are passed as JSON
 * strings the engine `JSON.stringify`s; the formatters return plain strings.
 * This mirrors `notes-engine.js`'s Qwen free-function convention.
 *
 * Privacy: no audio or model data crosses this surface — only the note/
 * transcript text the user is already copying/exporting, formatted in the Rust
 * WASM heap. Nothing leaves the browser.
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

export class ExportsEngine {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]  Override for the wasm-pack pkg URL.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;

    this._mod = null;
    this._loadPromise = null;
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
      console.log('[rust-exports] ExportsEngine ready (notes/transcript/summary markdown + history replay + duration)');
    })();

    return this._loadPromise;
  }

  /** @returns {object} the loaded wasm module (throws if not loaded). */
  _m() {
    if (!this._mod) throw new Error('[rust-exports] ExportsEngine not loaded — call load() first');
    return this._mod;
  }

  /**
   * `notesToMarkdown`: structured notes as Markdown (Appendix A row 30).
   * @param {string} title
   * @param {string} date      locale date string
   * @param {string} duration  mm:ss
   * @param {Array<{category:string, text:string, time:(string|null)}>} notes
   * @param {boolean} withTime  the `showTimestamps` toggle
   * @returns {string}
   */
  notesToMarkdown(title, date, duration, notes, withTime) {
    return this._m().notesToMarkdown(
      String(title || ''), String(date || ''), String(duration || ''),
      JSON.stringify(notes || []), !!withTime,
    );
  }

  /**
   * `openMeetingDetail` replay export: notes Markdown with NO per-line stamps
   * and no empty-text filter (Appendix A rows 29, 30).
   * @param {string} title
   * @param {string} date
   * @param {string} duration
   * @param {Array<{category:string, text:string}>} notes
   * @returns {string}
   */
  historyReplayMarkdown(title, date, duration, notes) {
    return this._m().historyReplayMarkdown(
      String(title || ''), String(date || ''), String(duration || ''),
      JSON.stringify(notes || []),
    );
  }

  /**
   * The meeting-summary executive line (`generateSummary`).
   * @param {string} duration
   * @param {Array<{category:string, text:string}>} notes
   * @param {number} totalWords
   * @returns {string}
   */
  executiveLine(duration, notes, totalWords) {
    return this._m().executiveLine(
      String(duration || ''), JSON.stringify(notes || []), Number(totalWords) || 0,
    );
  }

  /**
   * `copyTranscript`: timestamp-aware plain-text join (Appendix A row 30).
   * @param {Array<{time:string, text:string}>} lines
   * @param {boolean} withTime
   * @returns {string}
   */
  transcriptText(lines, withTime) {
    return this._m().transcriptText(JSON.stringify(lines || []), !!withTime);
  }

  /**
   * `copySummaryMarkdown`: append the additive AI Meeting Notes groups to a base
   * notes-Markdown document (Appendix A row 30).
   * @param {string} baseMd
   * @param {Array<{label:string, items:Array<{chip:(string|null), text:string}>}>} aiGroups
   * @returns {string}
   */
  summaryMarkdownWithAi(baseMd, aiGroups) {
    return this._m().summaryMarkdownWithAi(String(baseMd || ''), JSON.stringify(aiGroups || []));
  }

  /**
   * The history-list `Nm Ns` duration string (`formatDuration`, Appendix A
   * row 24).
   * @param {number} ms
   * @returns {string}
   */
  formatDuration(ms) {
    return this._m().formatDuration(Number(ms) || 0);
  }
}
