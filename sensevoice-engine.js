/**
 * sensevoice-engine.js — Rust/WASM-driven SenseVoice solo VAD-segmentation engine.
 *
 * The thin ES-module wrapper that REPLACES (strangler-fig) the inline
 * `SenseVoiceEngine.processAudio` policy in index.html (Appendix A row 11). The
 * SEGMENTATION policy — the 30 s circular-buffer windowing, the 512-sample VAD
 * window drain (strict `>`, holds one window back), and the per-segment `> 0.3 s`
 * decode gate — now lives in Rust as `SenseVoicePolicy` (crates/silent-inference),
 * surfaced as `WasmSenseVoice` in the shared `silent_web.js` pkg. This loader
 * drives that policy and the sherpa-onnx (js-sherpa) WASM harness, which is the
 * EXECUTOR: it runs the Silero VAD + the SenseVoice ASR decode. Same wasm binary,
 * same cross-loader init promise as the other engine loaders.
 *
 * # USER GATE — live SenseVoice witness is BLOCKED-ON-USER-GATE
 *
 * The sherpa-onnx + model artifacts are still pulled from the k2-fsa HuggingFace
 * Space, which is 401-dead (the Space was taken down). The wiring below points at
 * the SAME loader path the shipping app used (`SPACE_BASE`); a LIVE run will fail
 * to load the harness until Mike re-hosts the artifacts (the a5-sensevoice-prep
 * task prepped the upload). When that happens, the re-host is a ONE-LINE
 * `SPACE_BASE` change (below) — nothing else here changes.
 *
 * The POLICY is witnessed independently at the protocol level (commands in/out vs
 * the silent-inference goldens) — no live model required — via the headless test
 * harness; see the task notes. This file is structured so that protocol witness
 * exercises the exact `WasmSenseVoice` surface the live path uses.
 *
 * # What moved to Rust (the policy) vs what stays here (the executor)
 *
 *   Policy (Rust `WasmSenseVoice`):
 *     - the 512-sample VAD window drain (`while buffer.size() > windowSize`)
 *     - the per-segment `> 0.3 s` decode gate (drop sub-gate segments)
 *     - the VAD parameters as config the host applies (threshold, min-speech,
 *       min-silence, the 30 s max-speech window)
 *     - segment indexing + finalize/teardown semantics
 *
 *   Executor (this loader + the sherpa harness):
 *     - load the sherpa-onnx VAD + SenseVoice recognizer (host I/O)
 *     - feed the VAD a window (`vad.acceptWaveform`) per FeedVadWindow command
 *     - decode a gated segment (`recognizer.decode`) per DecodeSegment command
 */

// ── RE-HOST: change this ONE line when Mike re-hosts the sherpa artifacts ──
//   (a5-sensevoice-prep prepped the first-party HF upload, mirroring the TitaNet
//    re-host). Until then this Space path is 401-dead and a LIVE run cannot load.
const SPACE_BASE = 'https://huggingface.co/spaces/k2-fsa/web-assembly-vad-asr-sherpa-onnx-zh-en-ja-ko-cantonese-sense-voice/resolve/main';

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

/**
 * The js-sherpa host executor: owns the sherpa-onnx VAD + recognizer. It carries
 * NO policy (windowing + gate are Rust). The methods map 1:1 to the
 * `WasmSenseVoice` host commands:
 *   - FeedVadWindow  → feedWindow(samples)   → vad.acceptWaveform
 *   - DecodeSegment  → decode(samples)       → recognizer.decode → text
 *
 * This is the SAME sherpa load path the inline `SenseVoiceEngine` used; it is
 * factored out so the policy drives it. Live load is BLOCKED until SPACE_BASE is
 * re-hosted (see file header).
 */
class SherpaHost {
  constructor() {
    this.module = null;
    this.recognizer = null;
    this.vad = null;
    this.buffer = null;
    this.ready = false;
    this.onStatus = null;
  }

  async init() {
    if (this.ready) return;
    this.onStatus?.('Loading SenseVoice WASM module (~253MB)...', 0);
    await this._loadScript(`${SPACE_BASE}/sherpa-onnx-asr.js`);
    await this._loadScript(`${SPACE_BASE}/sherpa-onnx-vad.js`);

    return new Promise((resolve, reject) => {
      const moduleConfig = {
        locateFile: (path) => (path.endsWith('.wasm') || path.endsWith('.data')) ? `${SPACE_BASE}/${path}` : path,
        setStatus: (status) => {
          if (!status) return;
          const match = status.match(/Downloading data\.\.\. \((\d+)\/(\d+)\)/);
          if (match) {
            const loaded = Number(match[1]), total = Number(match[2]);
            const pct = total > 0 ? Math.round((loaded / total) * 100) : 0;
            const loadedMB = (loaded / (1024 * 1024)).toFixed(1), totalMB = (total / (1024 * 1024)).toFixed(1);
            this.onStatus?.(`Downloading SenseVoice... ${pct}% (${loadedMB}/${totalMB} MB)`, pct);
          } else if (status.startsWith('Running')) {
            this.onStatus?.('Initializing SenseVoice recognizer...', 95);
          } else {
            this.onStatus?.(status, null);
          }
        },
        onRuntimeInitialized: () => {
          try {
            this.module = moduleConfig;
            this._initRecognizer();
            this._initVAD();
            this.ready = true;
            this.onStatus?.('SenseVoice loaded — Rust policy drives windowing + 0.3s gate', 100);
            resolve();
          } catch (err) {
            reject(new Error('Failed to initialize SenseVoice: ' + err.message));
          }
        },
      };
      const savedModule = window.Module;
      window.Module = moduleConfig;
      const script = document.createElement('script');
      script.src = `${SPACE_BASE}/sherpa-onnx-wasm-main-vad-asr.js`;
      script.onerror = () => { window.Module = savedModule; reject(new Error('Failed to load sherpa-onnx WASM loader script (SPACE_BASE 401? re-host pending)')); };
      document.head.appendChild(script);
    });
  }

  _initRecognizer() {
    const config = { modelConfig: { debug: 0, tokens: './tokens.txt', senseVoice: { model: './sense-voice.onnx', useInverseTextNormalization: 1 } } };
    this.recognizer = new OfflineRecognizer(config, this.module);
  }

  /** The VAD config — its parameters mirror the Rust `SenseVoiceConfig::SHIPPING`. */
  _initVAD() {
    const vadConfig = {
      sileroVad: { model: './silero_vad.onnx', threshold: 0.5, minSpeechDuration: 0.25, minSilenceDuration: 0.5, maxSpeechDuration: 30, windowSize: 512 },
      sampleRate: 16000, debug: 0, numThreads: 1, bufferSizeInSeconds: 60,
    };
    this.vad = new Vad(vadConfig, this.module);
    this.buffer = new CircularBuffer(30 * 16000, this.module);
  }

  /** Append raw samples to sherpa's circular buffer (the host owns the samples). */
  push(samples) { if (this.ready && this.buffer) this.buffer.push(samples); }

  /** FeedVadWindow: feed exactly one 512-window to the VAD (host execution). */
  feedWindow() {
    if (!this.ready) return;
    const windowSize = this.vad.config.sileroVad.windowSize;
    if (this.buffer.size() <= windowSize) return;
    const windowSamples = this.buffer.get(this.buffer.head(), windowSize);
    this.vad.acceptWaveform(windowSamples);
    this.buffer.pop(windowSize);
  }

  /** Pop the next VAD-detected segment (or null). The policy gates on its length. */
  frontSegment() {
    if (!this.ready || this.vad.isEmpty()) return null;
    const seg = this.vad.front();
    return seg;
  }
  popSegment() { if (this.ready) this.vad.pop(); }

  /** DecodeSegment: run the SenseVoice ASR on the gated segment samples → text. */
  decode(segmentSamples) {
    const stream = this.recognizer.createStream();
    stream.acceptWaveform(16000, segmentSamples);
    this.recognizer.decode(stream);
    const result = this.recognizer.getResult(stream);
    const text = result.text.trim();
    stream.free();
    return text;
  }

  destroy() { this.ready = false; this.recognizer = null; this.vad = null; this.buffer = null; this.module = null; }

  _loadScript(url) {
    return new Promise((resolve, reject) => {
      const script = document.createElement('script');
      script.src = url; script.onload = resolve;
      script.onerror = () => reject(new Error('Failed to load: ' + url));
      document.head.appendChild(script);
    });
  }
}

export class SenseVoiceEngine {
  /**
   * @param {object} [opts]
   * @param {string}   [opts.pkgUrl]
   * @param {object}   [opts.host]   inject a host executor (the headless protocol
   *                                  witness passes a stub here; live path builds a
   *                                  SherpaHost). When omitted, a SherpaHost is built.
   */
  constructor(opts = {}) {
    const w = (typeof window !== 'undefined') ? window : {};
    this.pkgUrl = opts.pkgUrl || w.__SILENT_WEB_PKG_URL || w.__DIARIZATION_PKG_URL || DEFAULT_PKG_URL;
    this._injectedHost = opts.host || null;

    this.onResult = null;    // (text, segmentSamples) => void
    this.onStatus = null;    // (message|null, pct|null) => void

    this._mod = null;
    this._policy = null;     // WasmSenseVoice
    this._host = null;
    this.ready = false;
    this._loadPromise = null;
  }

  async init() {
    if (this._loadPromise) return this._loadPromise;
    this._loadPromise = (async () => {
      this._mod = await _loadModule(this.pkgUrl);
      this._policy = new this._mod.WasmSenseVoice();
      this._host = this._injectedHost || new SherpaHost();
      this._host.onStatus = (m, p) => this.onStatus?.(m, p);
      await this._host.init();
      this.ready = true;
      console.log('[rust-sensevoice] SenseVoiceEngine ready (Rust policy: 512-window drain + 0.3s decode gate; sherpa = VAD/ASR executor)');
    })();
    return this._loadPromise;
  }

  /**
   * Process captured 16 kHz samples. Drives the Rust policy: it returns the VAD
   * windows to feed; the host feeds the VAD; the host's detected segments are
   * gated by the policy (`> 0.3 s`) and the passing ones are decoded.
   * @param {Float32Array} samples
   */
  processAudio(samples) {
    if (!this.ready || !samples || !samples.length) return;
    // Host owns the samples (sherpa CircularBuffer); policy owns the windowing.
    this._host.push(samples);
    const cmds = JSON.parse(this._policy.pushSamples(samples.length));
    for (const cmd of cmds) {
      if (cmd.cmd === 'feed_vad_window') this._host.feedWindow();
    }
    // Drain any VAD-detected segments through the policy's decode gate.
    for (;;) {
      const seg = this._host.frontSegment();
      if (!seg) break;
      const res = JSON.parse(this._policy.onVadSegment(0, seg.samples.length));
      if (res.command && res.command.cmd === 'decode_segment') {
        const text = this._host.decode(seg.samples);
        if (text && text.length > 0) this.onResult?.(text, seg.samples);
      }
      this._host.popSegment();
    }
  }

  destroy() {
    this._policy?.requestStop();
    try { this._policy?.drainFinalize(); } catch (_) {}
    this._host?.destroy();
    this.ready = false;
  }

  reset() { this._policy?.reset(); }
}

export default SenseVoiceEngine;
