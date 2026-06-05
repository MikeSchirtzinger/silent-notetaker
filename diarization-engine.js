/**
 * diarization-engine.js — Rust/WASM speaker diarization engine.
 *
 * The thin ES-module wrapper around the wasm-pack build of `crates/silent-web`
 * (`crates/silent-web/pkg/`). It exposes a `DiarizationEngine` that the
 * `index.html` diarization path imports and drives — the same strangler-fig
 * pattern as `nemotron-engine.js` does for the ASR engine.
 *
 * The engine wraps:
 *   - `WasmTitaNetEmbedder`  (crates/silent-diarization, ort-web on the ONNX session)
 *   - `SpeakerTracker`       (crates/silent-diarization, pure Rust leader-clustering)
 *
 * Wire-up (in index.html, replaces the JS SpeakerEmbedder + SpeakerTracker):
 *
 *   const { DiarizationEngine } = await import('./diarization-engine.js');
 *   const diarz = new DiarizationEngine({ melFb: _TITANET_MEL_FB });
 *   diarz.onStatus = (msg) => updateUI(msg);
 *   await diarz.load();             // fetch TitaNet, build ORT session
 *   // on each utterance boundary:
 *   const info = await diarz.identify(samples);  // { id, name, color, isNew } | null
 *   // on speaker rename (click-to-edit):
 *   const outcome = diarz.evaluateRename(id, value);
 *   if (outcome.tag === 'merge') { if (confirm(...)) diarz.confirmMerge(from, to); }
 *   else                         { diarz.rename(id, name); }
 *   // at recording stop:
 *   const { relabel, speakers } = diarz.globalRecluster();
 *
 * Model artifacts:
 *   titanet.onnx (~40 MB) — fetched from the registry-pinned HF URL
 *                           (window.TITANET_URL override kept for offline use)
 *   mel_fb.json  (~24 KB) — inlined in index.html as `_TITANET_MEL_FB`
 *                           (E1 inlined it; passed via the `melFb` constructor option)
 *
 * Privacy: raw 192-d embeddings never leave the Rust WASM heap. Only speaker
 * labels (id, name, color, count) cross the wasm boundary. Verified by the
 * resource-list check in F2 validation.
 *
 * ort-web dist: uses the SAME cdn.pyke.io origin the app already uses today
 * (already in the generated CSP connect-src). Vendoring is a later phase.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

/**
 * Serialize a 2-D mel filterbank array (80 × 257, JS numbers) to UTF-8 JSON
 * bytes suitable for passing to `WasmDiarization.create(onnx_bytes, mel_fb_json)`.
 *
 * The Rust mel frontend (`silent_audio::parse_mel_fb_json`) expects the
 * `mel_fb.json` format: `{ "n_mels": 80, "n_freq": 257, "matrix": [[...]] }`.
 * The inlined JS constant `_TITANET_MEL_FB` is only the raw `[[...]]` array
 * (E1 extracted just the matrix, not the wrapper object), so we re-wrap it here.
 */
function melFbToBytes(melFb) {
  const obj = { n_mels: melFb.length, n_freq: melFb[0].length, matrix: melFb };
  return new TextEncoder().encode(JSON.stringify(obj));
}

export class DiarizationEngine {
  /**
   * @param {object} [opts]
   * @param {number[][]} [opts.melFb]      The 80×257 mel filterbank matrix (inlined
   *   in index.html as `_TITANET_MEL_FB`). If omitted, fetched from TITANET_MEL_FB_URL.
   * @param {string}     [opts.titanetUrl] Override for the TitaNet ONNX URL.
   * @param {string}     [opts.pkgUrl]     Override for the wasm-pack pkg URL.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.titanetUrl = opts.titanetUrl || w.TITANET_URL
      || 'https://huggingface.co/FluffyBunnies/titanet-small-onnx/resolve/5fae6d4e517a019cab845fd98935fd5b3776dfed/titanet.onnx';
    this.pkgUrl = opts.pkgUrl || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;
    // mel_fb: accept inlined JS array, or fetch from a URL at load time.
    this._melFb = opts.melFb || null;
    this._melFbUrl = opts.melFbUrl || w.TITANET_MEL_FB_URL || null;

    /** Status callback: `(message: string|null) => void` */
    this.onStatus = null;

    this._wasm = null;       // WasmDiarization instance
    this._loadPromise = null;
    this.ready = false;
    // Cached speaker list — kept in sync after every mutation so that
    // renderSpeakerTags() and maybeMergeOnRename() can read it synchronously.
    this._speakers = [];
  }

  /**
   * Load the TitaNet model and initialise the Rust diarization engine.
   * Idempotent — safe to call multiple times (returns the same promise after
   * the first call, so the shared-instance model-survive-meeting-reset
   * semantics of `sharedSpeakerEmbedder` are preserved).
   */
  load() {
    if (this.ready) return Promise.resolve();
    if (this._loadPromise) return this._loadPromise;

    this._loadPromise = (async () => {
      this.onStatus?.('Loading speaker model (TitaNet ~40MB, first time only)...');

      // Import + initialise the wasm-pack ES module via the SHARED cross-loader
      // promise (see session-engine.js): `mod.default()` must run exactly once
      // across all engine loaders, or a concurrent boot-time init double-
      // initializes the wasm binary and corrupts the heap. wasm-pack generates a
      // default export (init glue) + named exports for each #[wasm_bindgen] item.
      const mod = await (() => {
        const w = (typeof window !== 'undefined') ? window : globalThis;
        const cache = (w.__silentWebModulePromises ||= new Map());
        let p = cache.get(this.pkgUrl);
        if (!p) {
          p = (async () => { const m = await import(this.pkgUrl); await m.default(); return m; })();
          cache.set(this.pkgUrl, p);
        }
        return p;
      })();

      // Fetch TitaNet ONNX weights.
      this.onStatus?.('Fetching TitaNet ONNX session...');
      const onnxResp = await fetch(this.titanetUrl);
      if (!onnxResp.ok) {
        throw new Error(`[diarization] fetch TitaNet failed: ${onnxResp.status} ${onnxResp.statusText}`);
      }
      const onnxBytes = new Uint8Array(await onnxResp.arrayBuffer());

      // Resolve mel_fb bytes.
      let melFbBytes;
      if (this._melFb) {
        // Fast path: use the inlined JS array (passed from index.html).
        melFbBytes = melFbToBytes(this._melFb);
      } else if (this._melFbUrl) {
        const resp = await fetch(this._melFbUrl);
        if (!resp.ok) throw new Error(`[diarization] fetch mel_fb failed: ${resp.status}`);
        melFbBytes = new Uint8Array(await resp.arrayBuffer());
      } else {
        throw new Error('[diarization] no mel_fb source: pass melFb array or melFbUrl to constructor');
      }

      // Build the combined Rust diarization object (embedder + tracker).
      this._wasm = await mod.WasmDiarization.create(onnxBytes, melFbBytes);
      this._speakers = [];
      this.ready = true;
      this.onStatus?.('Speaker model ready');
      console.log('[rust-diarization] WasmDiarization ready (TitaNet + SpeakerTracker)');
    })();

    return this._loadPromise;
  }

  /**
   * Identify the speaker for a segment of 16 kHz mono PCM.
   *
   * - Segments shorter than 16 000 samples (1.0 s) reuse the last speaker
   *   (the `minSamples` branch in both JS and Rust — exact parity with the
   *   replaced `SpeakerTracker.identify()` too-short branch).
   * - Returns `{ id, name, color, isNew }` or `null` on error/no prior speaker.
   *
   * @param {Float32Array} samples  16 kHz mono f32 audio
   * @returns {Promise<{id:string, name:string, color:string, isNew:boolean}|null>}
   */
  async identify(samples) {
    if (!this.ready) {
      // Pre-warm didn't finish yet; degrade gracefully (no fake labels).
      return null;
    }
    const raw = await this._wasm.identify(samples);
    if (raw == null) return null;
    const info = JSON.parse(raw);
    // Keep the speaker cache current (new speaker was created).
    if (info.isNew) {
      this._speakers = JSON.parse(this._wasm.speakers());
    }
    return info;
  }

  /**
   * Evaluate whether a committed rename is a merge-by-rename.
   *
   * @param {string} fromId  The speaker being renamed
   * @param {string} value   The new name the user typed
   * @returns {{ tag: 'merge', payload: { from: string, target: string } }
   *          |{ tag: 'rename', payload: { id: string, name: string } }}
   */
  evaluateRename(fromId, value) {
    if (!this._wasm) return { tag: 'rename', payload: { id: fromId, name: value } };
    return JSON.parse(this._wasm.evaluate_rename(fromId, value));
  }

  /**
   * Apply a merge (user confirmed the merge-by-rename prompt, or an explicit
   * merge was requested). Folds `fromId` into `toId`.
   *
   * @returns {{ from_id: string, to_id: string }|null}  null on no-op
   */
  confirmMerge(fromId, toId) {
    if (!this._wasm) return null;
    const raw = this._wasm.confirm_merge(fromId, toId);
    const result = raw == null ? null : JSON.parse(raw);
    if (result) this._speakers = JSON.parse(this._wasm.speakers());
    return result;
  }

  /**
   * Apply a plain rename. Keeps the Rust tracker in sync with any DOM update
   * the UI has already made.
   *
   * @param {string} speakerId
   * @param {string} name
   */
  rename(speakerId, name) {
    if (!this._wasm) return;
    this._wasm.rename(speakerId, name);
    // Keep local cache in sync.
    const sp = this._speakers.find(s => s.id === speakerId);
    if (sp) sp.name = name;
  }

  /**
   * Run the stop-time global recluster (DIARIZATION.md §2, Appendix A row 15).
   *
   * Call once when recording stops, BEFORE showSummary(). The caller applies
   * the returned relabel map to the DOM (see applyRecluster in index.html).
   *
   * @param {number} [threshold]  Cosine merge threshold (default 0.65).
   * @returns {{ relabel: Array<{ old_id: string, new_id: string }>,
   *             speakers: Array<{ id: string, name: string, color: string, count: number }> }}
   */
  globalRecluster(threshold) {
    if (!this._wasm) return { relabel: [], speakers: [] };
    const th = (typeof threshold === 'number' && isFinite(threshold)) ? threshold : NaN;
    const result = JSON.parse(this._wasm.global_recluster(th));
    this._speakers = result.speakers;
    return result;
  }

  /**
   * Current speaker list. Used to rebuild the speakers bar after mutations.
   * Synchronous — returns the cached list (kept in sync by identify/merge/rename).
   *
   * @returns {Array<{ id: string, name: string, color: string, count: number }>}
   */
  get speakers() {
    return this._speakers;
  }

  /**
   * Reset the tracker for a new meeting while keeping the loaded ONNX session
   * alive — mirrors the `sharedSpeakerEmbedder` model-survive-meeting-reset
   * semantics. Called from newMeeting() in index.html.
   *
   * Returns `this` so the caller can keep the same reference.
   */
  reset() {
    if (this._wasm) this._wasm.reset_tracker();
    this._speakers = [];
    return this;
  }
}
