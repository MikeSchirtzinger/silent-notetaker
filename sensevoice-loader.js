/**
 * sensevoice-loader.js
 * Loads sherpa-onnx WASM module with SenseVoice from HuggingFace Space CDN
 * and provides a clean API for the main app.
 *
 * SenseVoice runs on the MAIN THREAD (not in a Web Worker) because the
 * Emscripten WASM module uses patterns incompatible with importScripts and
 * needs DOM access for script/data loading.
 *
 * Audio processing is fast (SenseVoice is non-autoregressive, encoder-only)
 * so main-thread execution is acceptable.
 *
 * The .data file (~241MB) bundles model weights, tokens, and Silero VAD.
 * Everything is loaded from the HuggingFace Space CDN — no local files needed.
 */

const SPACE_BASE = 'https://huggingface.co/spaces/k2-fsa/web-assembly-vad-asr-sherpa-onnx-zh-en-ja-ko-cantonese-sense-voice/resolve/main';

class SenseVoiceEngine {
  constructor() {
    this.module = null;
    this.recognizer = null;
    this.vad = null;
    this.buffer = null;
    this.ready = false;
    this.onResult = null;  // callback: (text) => void
    this.onStatus = null;  // callback: (message, progress) => void
  }

  async init() {
    if (this.ready) return;

    this.onStatus?.('Loading SenseVoice WASM module (~253MB)...', 0);

    // Load the sherpa-onnx API wrapper scripts first.
    // These define OfflineRecognizer, Vad, CircularBuffer, etc.
    await this._loadScript(`${SPACE_BASE}/sherpa-onnx-asr.js`);
    await this._loadScript(`${SPACE_BASE}/sherpa-onnx-vad.js`);

    // Now load the Emscripten WASM bundle. The loader script expects a global
    // `Module` object to be present at load time. We configure it with:
    //  - locateFile: redirect .wasm/.data fetches to the CDN
    //  - setStatus: parse download progress for user feedback
    //  - onRuntimeInitialized: called when WASM is fully ready
    return new Promise((resolve, reject) => {
      const moduleConfig = {
        locateFile: (path) => {
          if (path.endsWith('.wasm') || path.endsWith('.data')) {
            return `${SPACE_BASE}/${path}`;
          }
          return path;
        },

        setStatus: (status) => {
          if (!status) return; // empty string = done, handled in onRuntimeInitialized

          const match = status.match(/Downloading data\.\.\. \((\d+)\/(\d+)\)/);
          if (match) {
            const loaded = Number(match[1]);
            const total  = Number(match[2]);
            const pct = total > 0 ? Math.round((loaded / total) * 100) : 0;
            const loadedMB = (loaded / (1024 * 1024)).toFixed(1);
            const totalMB  = (total  / (1024 * 1024)).toFixed(1);
            this.onStatus?.(
              `Downloading SenseVoice... ${pct}% (${loadedMB}/${totalMB} MB)`,
              pct
            );
          } else if (status.startsWith('Running')) {
            this.onStatus?.('Initializing SenseVoice recognizer...', 95);
          } else {
            // Surface any other status messages so the user isn't left wondering
            this.onStatus?.(status, null);
          }
        },

        onRuntimeInitialized: () => {
          try {
            this.module = moduleConfig;
            this._initRecognizer();
            this._initVAD();
            this.ready = true;
            this.onStatus?.(
              'SenseVoice loaded — no 30s window, processes actual audio length',
              100
            );
            resolve();
          } catch (err) {
            reject(new Error('Failed to initialize SenseVoice: ' + err.message));
          }
        },
      };

      // Expose as window.Module so the Emscripten loader captures it.
      // We save and restore any pre-existing Module to be polite.
      const savedModule = window.Module;
      window.Module = moduleConfig;

      const script = document.createElement('script');
      script.src = `${SPACE_BASE}/sherpa-onnx-wasm-main-vad-asr.js`;
      script.onerror = () => {
        window.Module = savedModule;
        reject(new Error('Failed to load sherpa-onnx WASM loader script'));
      };
      document.head.appendChild(script);
      // NOTE: We intentionally do NOT restore window.Module after appending —
      // the Emscripten runtime holds a reference to the moduleConfig object and
      // continues to use it (e.g. for setStatus callbacks) after the script loads.
    });
  }

  _initRecognizer() {
    // OfflineRecognizer is defined by the loaded sherpa-onnx-asr.js script.
    // The .data bundle bakes in tokens.txt and sense-voice.onnx at the root path.
    const config = {
      modelConfig: {
        debug: 0,
        tokens: './tokens.txt',
        senseVoice: {
          model: './sense-voice.onnx',
          useInverseTextNormalization: 1,
        },
      },
    };

    this.recognizer = new OfflineRecognizer(config, this.module); // eslint-disable-line no-undef
  }

  _initVAD() {
    // Vad and CircularBuffer are defined by sherpa-onnx-vad.js.
    // silero_vad.onnx is also baked into the .data bundle.
    const vadConfig = {
      sileroVad: {
        model: './silero_vad.onnx',
        threshold: 0.5,
        minSpeechDuration: 0.25,
        minSilenceDuration: 0.5,
        maxSpeechDuration: 30,
        windowSize: 512,
      },
      sampleRate: 16000,
      debug: 0,
      numThreads: 1,
      bufferSizeInSeconds: 60,
    };

    this.vad = new Vad(vadConfig, this.module);           // eslint-disable-line no-undef
    this.buffer = new CircularBuffer(30 * 16000, this.module); // eslint-disable-line no-undef
  }

  /**
   * Feed a chunk of audio samples (Float32Array at 16 kHz) into the pipeline.
   * The Silero VAD segments speech; each complete segment is transcribed by
   * SenseVoice and delivered to onResult.
   *
   * @param {Float32Array} samples - PCM audio at 16 kHz, mono
   */
  processAudio(samples) {
    if (!this.ready || !this.vad || !this.buffer) return;

    // Accumulate samples in the circular buffer
    this.buffer.push(samples);

    // Feed VAD one window at a time
    const windowSize = this.vad.config.sileroVad.windowSize;
    while (this.buffer.size() > windowSize) {
      const windowSamples = this.buffer.get(this.buffer.head(), windowSize);
      this.vad.acceptWaveform(windowSamples);
      this.buffer.pop(windowSize);
    }

    // Drain any complete speech segments from the VAD
    while (!this.vad.isEmpty()) {
      const segment = this.vad.front();
      const durationSec = segment.samples.length / 16000;

      if (durationSec > 0.3) {
        // Run SenseVoice on this segment
        const stream = this.recognizer.createStream();
        stream.acceptWaveform(16000, segment.samples);
        this.recognizer.decode(stream);
        const result = this.recognizer.getResult(stream);
        const text = result.text.trim();
        stream.free();

        if (text.length > 0) {
          this.onResult?.(text, segment.samples);
        }
      }

      this.vad.pop();
    }
  }

  /**
   * Release all resources. Call when stopping recording.
   */
  destroy() {
    this.ready = false;
    // The WASM module manages its own memory; we just drop references.
    this.recognizer = null;
    this.vad = null;
    this.buffer = null;
    this.module = null;
  }

  _loadScript(url) {
    return new Promise((resolve, reject) => {
      const script = document.createElement('script');
      script.src = url;
      script.onload = resolve;
      script.onerror = () => reject(new Error('Failed to load: ' + url));
      document.head.appendChild(script);
    });
  }
}
