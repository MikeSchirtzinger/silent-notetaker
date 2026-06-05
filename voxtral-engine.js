/**
 * voxtral-engine.js — Rust/WASM-driven Voxtral two-cap recycle engine.
 *
 * The thin ES-module wrapper that REPLACES (strangler-fig) the inline
 * `_runVoxtralTranscription` policy loop in index.html (Appendix A row 10). The
 * hardest-won bug fix in the app — Voxtral's token/audio TWO-CAP context recycle
 * and the in-place partial-text machine (`flushDecodedText` / `streamer.end`) —
 * now lives in Rust as `VoxtralRecyclePolicy` (crates/silent-inference), surfaced
 * as `WasmVoxtralRecycle` in the shared `silent_web.js` pkg. This loader drives
 * that policy; the transformers.js model (`model.generate(...)`), the mel feature
 * extraction, and the fixed-footprint ring buffer stay here as the EXECUTOR (host
 * I/O + the actual decode — keeping them in JS preserves the measured streaming
 * hot-path baseline; the b2 spike's no-regression gate). Same wasm binary, same
 * cross-loader init promise as the other engine loaders.
 *
 * # What moved to Rust (the policy) vs what stays here (the executor)
 *
 *   Policy (Rust `WasmVoxtralRecycle`):
 *     - the TOKEN CAP (320 tokens) and the AUDIO/TIME CAP (45 s) — the two
 *       independent caps that bound each generate context's KV+arena memory
 *     - the recycle decision + seam (a fresh context anchored at the current ring
 *       write position — no skip, no re-read)
 *     - the in-place partial-text machine: the printLen/sentenceBuffer slicing
 *       that produces onPartial(sentenceBuffer) per delta and onFinal(sentence) on
 *       each sentence-ending-punctuation boundary (in-place partial semantics, row 10)
 *     - the bounded-growth session stats (the sawtooth Diag trail)
 *
 *   Executor (this loader):
 *     - load the Voxtral model + processor (host I/O; download progress)
 *     - the Float32 ring buffer + mel feature extraction (`processor(...)`)
 *     - `model.generate({ input_features, max_new_tokens, streamer })` — the decode
 *     - the streamer that reports token deltas + cumulative decode back to the policy
 *
 * # In-place partials preserved (row 10)
 *
 * The live transcript element is updated IN PLACE on every `onPartial` and
 * promoted to a final segment on every `onFinal`, exactly as before — the policy
 * emits the same `Partial`/`Final` text the JS `flushDecodedText` did. index.html
 * keeps its `onStreamText`/`onSentenceComplete` render path unchanged.
 */

const DEFAULT_PKG_URL = new URL('./crates/silent-web/pkg/silent_web.js', import.meta.url).href;
const VOXTRAL_TFJS_URL = 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@4.0.0-next.7/dist/transformers.min.js';
const MODEL_ID = 'onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX';

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

export class VoxtralEngine {
  /**
   * @param {object} [opts]
   * @param {string}   [opts.pkgUrl]
   * @param {object}   [opts.testCaps]  { maxNewTokens, maxCtxSamples } — forces a
   *                                     small-cap recycle for the I5 witness run.
   *                                     OMIT for shipping (320 / 45 s caps).
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;
    this.testCaps = opts.testCaps || null;

    this.onPartial = null;   // (fullText) => void  — in-place live element
    this.onFinal   = null;   // (sentence) => void  — promote to final segment
    this.onStatus  = null;   // (message|null, pct|null) => void
    this.onRecycle = null;   // (reason, stats) => void  — observable recycle (witness)

    this._mod = null;
    this._policy = null;     // WasmVoxtralRecycle
    this.model = null;
    this.processor = null;
    this._BaseStreamer = null;
    this.ready = false;
    this._loadPromise = null;
    this._runId = 0;
    this._stopRequested = false;
    this._loopPromise = null;
  }

  /** Load the wasm policy + the Voxtral model/processor. */
  load() {
    if (this._loadPromise) return this._loadPromise;
    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      const { WasmVoxtralRecycle } = this._mod;
      this._policy = this.testCaps
        ? WasmVoxtralRecycle.withTestCaps(this.testCaps.maxNewTokens, this.testCaps.maxCtxSamples)
        : new WasmVoxtralRecycle();

      this.onStatus?.('Loading Voxtral model (~2.7GB, first load only)...', 10);
      const tfjs = await import(/* @vite-ignore */ VOXTRAL_TFJS_URL);
      this._BaseStreamer = tfjs.BaseStreamer;
      const { VoxtralRealtimeForConditionalGeneration, VoxtralRealtimeProcessor } = tfjs;

      const progressCallback = (progress) => {
        if (progress.status === 'downloading') {
          const pct = progress.total > 0 ? Math.round(10 + (progress.loaded / progress.total) * 75) : 10;
          const mb = progress.loaded ? (progress.loaded / 1048576).toFixed(0) + 'MB' : '';
          this.onStatus?.(`Downloading Voxtral: ${progress.file || ''} ${mb}`, pct);
        } else if (progress.status === 'loading') {
          this.onStatus?.('Loading Voxtral weights...', 85);
        } else if (progress.status === 'ready') {
          this.onStatus?.('Voxtral ready — streaming transcription active', 100);
        }
      };

      this.model = await VoxtralRealtimeForConditionalGeneration.from_pretrained(MODEL_ID, {
        dtype: { audio_encoder: 'q4f16', embed_tokens: 'q4f16', decoder_model_merged: 'q4f16' },
        device: 'webgpu',
        progress_callback: progressCallback,
      });
      this.processor = await VoxtralRealtimeProcessor.from_pretrained(MODEL_ID);
      this.onStatus?.('Voxtral ready — streaming transcription active', 100);
      this.ready = true;
      console.log(`[rust-voxtral] VoxtralEngine ready (Rust policy: two-cap recycle + in-place partials${this.testCaps ? ` — TEST caps ${this.testCaps.maxNewTokens} tok / ${this.testCaps.maxCtxSamples} samples` : ' — VOXTRAL_SHIPPING 320 tok / 45 s'})`);
    })();
    return this._loadPromise;
  }

  /**
   * Start the streaming transcription loop against a ring buffer. The loop runs
   * until `stop()` (or a newer run supersedes it). Drives the Rust recycle policy:
   * each `poll` returns the next host command (start a generate context, recycle,
   * or finalize); the host executes it and reports token/audio/text progress back.
   *
   * @param {object} ring  Float32RingBuffer (writeAbs, earliest, read(a,b), reset())
   */
  start(ring) {
    this._stopRequested = false;
    const myRunId = ++this._runId;
    const isStop = () => this._stopRequested || this._runId !== myRunId;
    this._policy.reset?.();   // fresh policy state for this run
    this._loopPromise = this._runLoop(ring, isStop).catch((err) => {
      if (!isStop()) { console.error('[Voxtral] loop error', err); this.onStatus?.('Voxtral error: ' + (err.message || err), null); }
    });
    return this._loopPromise;
  }

  /** Request the loop to stop and await its termination. */
  async stop() {
    this._stopRequested = true;
    this._runId++;
    if (this._policy) this._policy.requestStop();
    const p = this._loopPromise;
    if (p) { this._loopPromise = null; try { await p; } catch (_) {} }
  }

  /** The bounded-growth session stats (sawtooth proof for the I5 witness). */
  sessionStats() {
    return this._policy ? JSON.parse(this._policy.sessionStats()) : null;
  }

  // ── the host execution loop (policy decides; host executes) ──
  async _runLoop(ring, isStop) {
    const processor = this.processor;
    const model = this.model;
    const BaseStreamer = this._BaseStreamer;
    const policy = this._policy;
    const absLen = () => ring.writeAbs;
    const numSamplesFirst = processor.num_samples_first_audio_chunk;
    const { hop_length, n_fft } = processor.feature_extractor.config;
    const winHalf = Math.floor(n_fft / 2);
    const samplesPerTok = processor.audio_length_per_tok * hop_length;
    const tokenizer = processor.tokenizer;
    const specialIds = new Set(tokenizer.all_special_ids.map(BigInt));
    const dispose = (t) => { try { if (t && typeof t.dispose === 'function') t.dispose(); } catch (_) {} };
    const waitUntil = (condition) => new Promise(resolve => {
      if (condition()) return resolve();
      const interval = setInterval(() => { if (condition()) { clearInterval(interval); resolve(); } }, 50);
    });
    const ms = (abs) => Math.round(abs * 1000 / 16000);

    // Outer loop: each iteration is one `poll`-driven generate context. The policy
    // owns when to start, when (and why) to recycle, and the finalize.
    while (!isStop()) {
      const startCmd = JSON.parse(policy.poll(ring.writeAbs, 0));
      if (!startCmd) break;                     // policy stopped → finalized
      if (startCmd.cmd === 'finalize') break;   // stop arrived between polls
      if (startCmd.cmd !== 'start_context') {
        // A recycle returned without an active context — re-poll to open the seam.
        continue;
      }

      const baseAbs = startCmd.anchor_abs;
      const maxNewTokens = startCmd.max_new_tokens;

      await waitUntil(() => absLen() >= baseAbs + numSamplesFirst || isStop());
      if (isStop()) break;

      let firstChunkInputs;
      try {
        firstChunkInputs = await processor(ring.read(baseAbs, baseAbs + numSamplesFirst), { is_streaming: true, is_first_audio_chunk: true });
      } catch (_) { continue; }   // ring moved under us → re-poll

      // Report the real prompt token count to the policy.
      try {
        const ii = firstChunkInputs.input_ids;
        const inTok = ii && ii.dims ? ii.dims[ii.dims.length - 1] : 0;
        policy.onContextStarted(inTok);
      } catch (_) { policy.onContextStarted(0); }

      let tokenCache = [];
      let isPrompt = true;
      let lastConsumedAbs = baseAbs;

      const flushText = () => {
        if (tokenCache.length === 0) return;
        const decoded = tokenizer.decode(tokenCache, { skip_special_tokens: true });
        const events = JSON.parse(policy.onDecodedText(decoded, ms(baseAbs), ms(lastConsumedAbs)));
        for (const ev of events) {
          if (ev.tag === 'partial') this.onPartial?.(ev.payload.text);
          else if (ev.tag === 'final') this.onFinal?.(ev.payload.text);
        }
      };

      const self = this;
      const streamer = new (class extends BaseStreamer {
        put(value) {
          if (isStop()) return;
          if (isPrompt) { isPrompt = false; return; }
          const tokens = value[0];
          if (tokens.length === 1 && specialIds.has(tokens[0])) return;
          policy.onTokens(tokens.length);   // the policy applies the token cap on the next poll
          tokenCache = tokenCache.concat(Array.from ? Array.from(tokens) : [...tokens]);
          flushText();
        }
        end() {
          if (isStop()) { tokenCache = []; isPrompt = true; return; }
          flushText();
          const tail = JSON.parse(policy.onContextEndText(ms(baseAbs), ms(lastConsumedAbs)));
          for (const ev of tail) if (ev.tag === 'final') self.onFinal?.(ev.payload.text);
          tokenCache = []; isPrompt = true;
        }
      })();

      // Mel feature generator — the AUDIO/TIME cap is enforced by polling the policy.
      async function* inputFeaturesGenerator() {
        let pending = firstChunkInputs.input_features;
        yield pending;
        let melFrameIdx = processor.num_mel_frames_first_audio_chunk;
        let startIdx = baseAbs + melFrameIdx * hop_length - winHalf;

        while (!isStop()) {
          // Report audio consumed; ask the policy if a cap tripped (audio OR token).
          policy.onAudioAdvanced(baseAbs, startIdx);
          lastConsumedAbs = startIdx;
          const capCmd = JSON.parse(policy.poll(ring.writeAbs, 0));
          if (capCmd && capCmd.cmd === 'recycle') {
            if (self.onRecycle) self.onRecycle(capCmd.reason, capCmd.stats);
            break;   // generate() returns; the outer while opens the seam context
          }
          // A non-recycle poll here would have advanced the policy state machine
          // wrongly, so the policy only returns recycle/none in Running — `null`
          // means keep streaming.

          const endNeeded = startIdx + processor.num_samples_per_audio_chunk;
          await waitUntil(() => absLen() >= endNeeded || isStop());
          if (isStop()) break;

          let batchEndSample = endNeeded;
          while (batchEndSample + samplesPerTok <= absLen()) batchEndSample += samplesPerTok;

          let chunkInputs;
          try {
            chunkInputs = await processor(ring.read(startIdx, batchEndSample), { is_streaming: true, is_first_audio_chunk: false });
          } catch (_) { break; }

          if (pending !== chunkInputs.input_features) dispose(pending);
          pending = chunkInputs.input_features;
          yield pending;

          melFrameIdx += chunkInputs.input_features.dims[2];
          startIdx = baseAbs + melFrameIdx * hop_length - winHalf;
        }
        dispose(pending);
      }

      try {
        await model.generate({
          input_ids: firstChunkInputs.input_ids,
          input_features: inputFeaturesGenerator(),
          max_new_tokens: maxNewTokens,
          streamer,
        });
      } finally {
        dispose(firstChunkInputs.input_ids);
      }
      // generate() returned (token cap OR audio cap OR stop). If it was the token
      // cap (the policy did not already recycle inside the generator), recycle now.
      if (!isStop() && policy.isRunning()) {
        const recycleCmd = JSON.parse(policy.poll(ring.writeAbs, 0));
        if (recycleCmd && recycleCmd.cmd === 'recycle' && this.onRecycle) {
          this.onRecycle(recycleCmd.reason, recycleCmd.stats);
        }
      }
    }
  }
}

export default VoxtralEngine;
