/**
 * storage-engine.js — Rust/WASM browser-storage engine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes a `StorageEngine` that `index.html`
 * drives for every persistence operation — the strangler-fig replacement for the
 * Dexie (`db.*`) calls, the same pattern as `session-engine.js`,
 * `notes-engine.js`, and `diarization-engine.js`. It loads the SAME
 * `silent_web.js` pkg module the other engines load (one wasm binary, many
 * surfaces) via the shared cross-loader init promise.
 *
 * It wraps the `silent-storage` policy (PRD Phase 4, Appendix A rows 1, 3, 16,
 * 17, 19, 26, 27, 29, 33, plus the Phase-F durable-speaker-names carry-forward):
 *
 *   - LIVE CRUD that replaces every Dexie call:
 *       meetings:         addMeeting / updateMeetingEnd
 *       transcriptChunks: addTranscriptChunk
 *       notes:            addNote / updateNoteText / updateNoteCategory / deleteNote
 *       screenshots:      addScreenshot / markScreenshotAnalyzed / countScreenshots
 *       history:          recentMeetings / meetingDetail
 *   - MIGRATION: migrateDatabase (Dexie v2 → Rust zero-loss, export-backup first).
 *   - DURABLE SPEAKER NAMES: saveSpeakerNames / loadSpeakerNames (Phase-F).
 *
 * The Rust core owns the law: the IndexedDB access, the schema ownership (it now
 * opens the DB at v3 and adds the `speakerNames` store), the zero-loss migration,
 * and the export-backup-before-write guarantee. index.html keeps the hands: it
 * renders, and on migration it wires the `backup_ready` event to its existing
 * download path so the user saves a backup BEFORE any migration write.
 *
 * Privacy: this is the ONLY persistence surface; nothing leaves the browser. The
 * Dexie unpkg `<script>` is removed from the app — which also shrinks the CSP's
 * connect-src (a K2 follow-up: `https://unpkg.com` is no longer needed).
 *
 * # ids
 *
 * The wasm `add_*` functions return the IndexedDB auto-increment key as a JS
 * number (the same numeric id Dexie returned), so every `db.notes.add(...).then(id
 * => ...)` call site keeps working with the id unchanged.
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

export class StorageEngine {
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
      console.log('[rust-storage] StorageEngine ready (live CRUD + migration + durable speaker names)');
    })();

    return this._loadPromise;
  }

  /** @returns {object} the loaded wasm module (throws if not loaded). */
  _m() {
    if (!this._mod) throw new Error('[rust-storage] StorageEngine not loaded — call load() first');
    return this._mod;
  }

  /**
   * Read all four tables as a verification summary (counts + per-table arrays +
   * screenshot encodings + raw bytes). Used by the before/after zero-loss proof
   * and smoke checks.
   * @returns {Promise<object>}
   */
  readDatabaseSummary() {
    return this._m().read_database_summary();
  }

  // ── meetings (Appendix A rows 1, 3, 33) ─────────────────────────────────────

  /**
   * `db.meetings.add({ title, startTime, endTime:null, duration:0 })`.
   * @param {string} title
   * @param {number} startTime  epoch ms
   * @returns {Promise<number>} new meeting id
   */
  addMeeting(title, startTime) {
    return this._m().add_meeting(String(title || ''), startTime);
  }

  /**
   * `db.meetings.update(id, { endTime, duration })` (the Stop write).
   * @param {number} meetingId
   * @param {number} endTime
   * @param {number} duration
   * @returns {Promise<void>}
   */
  updateMeetingEnd(meetingId, endTime, duration) {
    return this._m().update_meeting_end(Number(meetingId), endTime, duration);
  }

  // ── transcriptChunks (Appendix A rows 29, 33) ───────────────────────────────

  /**
   * `db.transcriptChunks.add({ meetingId, timestamp, text, isFinal:true })`.
   * @returns {Promise<number>} new chunk id
   */
  addTranscriptChunk(meetingId, timestamp, text) {
    return this._m().add_transcript_chunk(Number(meetingId), timestamp, String(text || ''));
  }

  // ── notes (Appendix A rows 16, 17, 19, 33) ──────────────────────────────────

  /**
   * `db.notes.add({ meetingId, category, text, timestamp, triggerPhrase })`.
   * @returns {Promise<number>} new note id
   */
  addNote(meetingId, category, text, timestamp, triggerPhrase) {
    return this._m().add_note(
      Number(meetingId), String(category || ''), String(text || ''),
      Number(timestamp) || 0, String(triggerPhrase || ''),
    );
  }

  /** `db.notes.update(id, { text })` (row 17 edit). @returns {Promise<void>} */
  updateNoteText(noteId, text) {
    return this._m().update_note_text(Number(noteId), String(text || ''));
  }

  /** `db.notes.update(id, { category })` (row 17 recategorize). @returns {Promise<void>} */
  updateNoteCategory(noteId, category) {
    return this._m().update_note_category(Number(noteId), String(category || ''));
  }

  /** `db.notes.delete(id)` (row 17 delete). @returns {Promise<void>} */
  deleteNote(noteId) {
    return this._m().delete_note(Number(noteId));
  }

  // ── screenshots (Appendix A rows 26, 27, 33) ────────────────────────────────

  /**
   * `db.screenshots.add(...)`. The live capture path passes the base64 data-URL
   * STRING; we store it as UTF-8 bytes with `encoding:'base64'` (the Rust-owned
   * normalized layout — indistinguishable from a migrated base64 row, so the
   * render path is uniform).
   *
   * @param {number} meetingId
   * @param {number} timestamp
   * @param {string|Uint8Array} image  base64 data-URL string (live) or raw bytes
   * @param {number} width
   * @param {number} height
   * @returns {Promise<number>} new screenshot id
   */
  addScreenshot(meetingId, timestamp, image, width, height) {
    let bytes;
    let encoding;
    if (typeof image === 'string') {
      bytes = new TextEncoder().encode(image);
      encoding = 'base64';
    } else {
      bytes = image instanceof Uint8Array ? image : new Uint8Array(image);
      encoding = 'bytes';
    }
    return this._m().add_screenshot(Number(meetingId), timestamp, bytes, encoding, width >>> 0, height >>> 0);
  }

  /**
   * `db.screenshots.where('timestamp').equals(ts).modify({ analyzed, analysis })`.
   * @returns {Promise<number>} rows updated
   */
  markScreenshotAnalyzed(timestamp, analysis) {
    return this._m().mark_screenshot_analyzed(timestamp, String(analysis || ''));
  }

  /**
   * `db.screenshots.where('meetingId').equals(id).count()`.
   * @returns {Promise<number>}
   */
  countScreenshots(meetingId) {
    return this._m().count_screenshots(Number(meetingId));
  }

  // ── history (Appendix A rows 29, 30) ────────────────────────────────────────

  /**
   * Last-50 meetings newest-first
   * (`db.meetings.orderBy('startTime').reverse().limit(50)`). Ranked by the
   * `silent-storage::search` policy — the same function the fuzzy search filters
   * on, so the initial and filtered lists rank identically (Appendix A row 29).
   * @returns {Promise<Array<{id:number,title:string,startTime:number,endTime:(number|null),duration:number}>>}
   */
  recentMeetings() {
    return this._m().recent_meetings();
  }

  /**
   * Fuzzy-search the meeting history (Appendix A row 29): case-insensitive
   * substring across title → notes → transcript chunks, within the last-50
   * newest-first window. Returns the matched meetings in display order — the
   * SAME row shape as {@link recentMeetings}. An empty/whitespace query returns
   * the full recent list. Runs the `silent-storage::search` policy in ONE DB
   * read (the JS N+1 per-meeting detail reads are gone).
   * @param {string} query
   * @returns {Promise<Array<{id:number,title:string,startTime:number,endTime:(number|null),duration:number}>>}
   */
  searchHistory(query) {
    return this._m().search_history(String(query || ''));
  }

  /**
   * One meeting's detail for replay export
   * (`{ meeting, notes, chunks }`).
   * @param {number} meetingId
   * @returns {Promise<{meeting:object|null, notes:object[], chunks:object[]}>}
   */
  meetingDetail(meetingId) {
    return this._m().meeting_detail(Number(meetingId));
  }

  // ── durable speaker names (Phase-F carry-forward) ───────────────────────────

  /**
   * Persist a meeting's speaker rename map so renames survive reload.
   * @param {number} meetingId
   * @param {Object<string,string>} names  raw id ("S1") → assigned name
   * @returns {Promise<void>}
   */
  saveSpeakerNames(meetingId, names) {
    return this._m().save_speaker_names(Number(meetingId), JSON.stringify(names || {}));
  }

  /**
   * Load a meeting's speaker rename map (`{}` if none).
   * @param {number} meetingId
   * @returns {Promise<Object<string,string>>}
   */
  async loadSpeakerNames(meetingId) {
    const json = await this._m().load_speaker_names(Number(meetingId));
    try { return JSON.parse(json) || {}; } catch { return {}; }
  }
}
