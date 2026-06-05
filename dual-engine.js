/**
 * dual-engine.js — Rust/WASM-driven Dual-mode (Moonshine drafts + SenseVoice refine).
 *
 * The thin ES-module wrapper that REPLACES (strangler-fig) the inline Dual-mode
 * interleaving in index.html (`startDualModel` + the Dual branches of
 * `handlePartial`/`handleFinal`; Appendix A row 11). The draft/refine COORDINATION
 * — append a Moonshine final as a draft, and on a SenseVoice refined final
 * supersede the older drafts (keep at most one as a preview) then append the
 * refined item — now lives in Rust as `DualCoordinator` (crates/silent-inference),
 * surfaced as `WasmDual` in the shared `silent_web.js` pkg.
 *
 * It composes the two leg engines, each itself Rust-policy-driven:
 *   - the Moonshine leg = `WhisperEngine({ dualLeg: true })` (3 s chunks, the
 *     `WasmWhisperStream` MOONSHINE_DUAL cadence). Its finals are DRAFTS.
 *   - the SenseVoice leg = `SenseVoiceEngine` (the `WasmSenseVoice` VAD policy).
 *     Its finals are REFINED.
 *
 * Both legs feed the SAME audio; this coordinator interleaves their outputs into
 * the transcript-item list via `WasmDual`, emitting `ListEdit`s the UI applies.
 *
 * # USER GATE — the SenseVoice leg is BLOCKED-ON-USER-GATE
 *
 * The SenseVoice leg loads sherpa-onnx from the 401-dead k2-fsa Space (see
 * sensevoice-engine.js). A LIVE Dual run cannot load the refiner until Mike
 * re-hosts those artifacts (one-line SPACE_BASE change). The Moonshine leg + the
 * Rust coordinator are witnessable today; the SenseVoice leg is BLOCKED there.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;

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

export class DualEngine {
  /**
   * @param {object} [opts]
   * @param {string} [opts.pkgUrl]
   * @param {object} [opts.senseVoiceHost]  optional injected sherpa host (witness).
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;
    this._senseVoiceHost = opts.senseVoiceHost || null;

    // Callbacks: the UI applies the ListEdits (append draft / remove draft /
    // append refined) to the transcript list. `onResult` carries the segment
    // samples for the speaker-tracker (same as the inline SenseVoice path).
    this.onListEdits = null;   // (edits) => void
    this.onStatus    = null;   // (message|null, pct|null) => void
    this.onSegment   = null;   // (text, segmentSamples) => void  — for speaker id

    this._mod = null;
    this._coordinator = null;   // WasmDual
    this.moonshine = null;      // WhisperEngine (dualLeg)
    this.senseVoice = null;     // SenseVoiceEngine
    this.ready = false;
    this._loadPromise = null;
  }

  async init() {
    if (this._loadPromise) return this._loadPromise;
    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this._coordinator = new this._mod.WasmDual();

      // ── Moonshine leg (instant drafts) ──
      this.onStatus?.('Loading Moonshine for real-time drafts...', 10);
      const { WhisperEngine } = await import('./whisper-engine.js');
      this.moonshine = new WhisperEngine({ pkgUrl: this.pkgUrl, dualLeg: true });
      this.moonshine.onStatus = (m, p) => this.onStatus?.('Moonshine: ' + (m || ''), p);
      this.moonshine.onFinal = (text) => this._onMoonshineFinal(text);
      await this.moonshine.load({ model: 'onnx-community/moonshine-base-ONNX', dtype: 'fp32', device: 'webgpu' });

      // ── SenseVoice leg (accurate refinement) — BLOCKED-ON-USER-GATE for live ──
      const { SenseVoiceEngine } = await import('./sensevoice-engine.js');
      this.senseVoice = new SenseVoiceEngine({ pkgUrl: this.pkgUrl, host: this._senseVoiceHost || undefined });
      this.senseVoice.onStatus = (m, p) => this.onStatus?.(m, (p !== null && p !== undefined) ? 30 + Math.round(p * 0.7) : p);
      this.senseVoice.onResult = (text, samples) => this._onSenseVoiceFinal(text, samples);
      await this.senseVoice.init();

      this.ready = true;
      this.onStatus?.('✓ Dual-model active — Moonshine (instant) + SenseVoice (refined)', 100);
      console.log('[rust-dual] DualEngine ready (Rust coordinator: draft/refine supersede; both legs Rust-policy-driven)');
    })();
    return this._loadPromise;
  }

  /** Feed captured samples to BOTH legs (each runs its own Rust policy). */
  feed(samples) {
    if (!this.ready) return;
    this.senseVoice?.processAudio(samples);
    this.moonshine?.feed(samples);
  }

  async stop() {
    await this.moonshine?.stop();
    this.senseVoice?.destroy();
    this.ready = false;
  }

  reset() {
    this._coordinator?.reset();
    this.moonshine?.reset();
    this.senseVoice?.reset();
  }

  _onMoonshineFinal(text) {
    const edits = JSON.parse(this._coordinator.onMoonshineFinal(text));
    if (edits.length) this.onListEdits?.(edits);
  }

  _onSenseVoiceFinal(text, samples) {
    const edits = JSON.parse(this._coordinator.onSenseVoiceFinal(text));
    if (edits.length) this.onListEdits?.(edits);
    // The refined segment samples drive speaker identification (same as solo).
    if (this.onSegment) this.onSegment(text, samples);
  }
}

export default DualEngine;
