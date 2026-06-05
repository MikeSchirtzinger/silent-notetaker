/**
 * transformers-host.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * Role: the `js-transformers` model host worker. It receives typed commands from
 * Rust (load/feed/generate/recycle) and returns events; it contains NO policy:
 * chunk sizes, Voxtral's two-cap recycle, when to feed, and when to recycle are
 * decided in Rust and arrive as typed commands; the worker returns events.
 * Reviewers can verify the absence of policy by reading this file (PRD R2
 * acceptance).
 *
 * The boundary shape and its negligible hot-path cost are proven in
 * docs/research/spike-jshost.md (GATE: PASS — no measurable latency regression;
 * keep the transferable discipline for audio/tensor payloads). Lands in Phase 5.
 *
 * # Current content (Phase 1 relocation)
 *
 * `EXECUTOR_WORKER_SRC` is the source text for the ASR pipeline executor worker
 * that `whisper-engine.js` previously defined inline. The worker:
 *   - loads the transformers.js ASR pipeline (host I/O; model download progress)
 *   - runs `transcriber(audio)` per chunk, replying with the RAW decoded text
 *   - carries NO policy (VAD / hallucination / dedup / chunk boundaries — Rust)
 *
 * The main thread posts `{ type: 'transcribe', chunk, audio }` (Float32Array);
 * the worker replies `{ type: 'decoded', chunk, text }`. Model-load lifecycle
 * messages (status/progress/ready/error) mirror what the index.html UI already
 * renders. Supports configure/init/transcribe/terminate message types.
 *
 * The Voxtral host (voxtral-engine.js) loads transformers.js directly on the
 * main thread (the mel-feature generator + model.generate loop are host I/O that
 * must stay on the main thread for the b2-spike latency gate). That path does NOT
 * use this worker — it imports transformers.js via a direct `import()` call. Both
 * are host executors; neither owns policy.
 */

/**
 * Source text for the ASR executor blob-worker (previously inlined in
 * whisper-engine.js). Instantiate via:
 *
 *   const url = URL.createObjectURL(new Blob([EXECUTOR_WORKER_SRC], { type: 'application/javascript' }));
 *   const worker = new Worker(url, { type: 'module' });
 *   URL.revokeObjectURL(url);
 *
 * The worker protocol:
 *   Main → Worker:  { type: 'configure', config: { model, dtype, device, language } }
 *                   { type: 'init' }
 *                   { type: 'transcribe', chunk: number, audio: Float32Array }
 *                   { type: 'terminate' }
 *   Worker → Main:  { type: 'status',   message: string }
 *                   { type: 'progress', value: number, file?: string }
 *                   { type: 'ready' }
 *                   { type: 'error',    message: string }
 *                   { type: 'decoded',  chunk: number, text: string }
 */
export const EXECUTOR_WORKER_SRC = `
import { pipeline, env } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3/dist/transformers.min.js';

env.allowLocalModels = false;
env.useBrowserCache = true;

let transcriber = null;
let isInitializing = false;
let config = {
  model: 'onnx-community/whisper-large-v3-turbo',
  dtype: 'q4f16',
  device: 'webgpu',
  language: 'en',
};

async function detectBestDevice() {
  try {
    if (typeof navigator !== 'undefined' && navigator.gpu) {
      const adapter = await navigator.gpu.requestAdapter();
      if (adapter) { self.postMessage({ type: 'status', message: 'WebGPU available — using GPU acceleration' }); return 'webgpu'; }
    }
  } catch (_) {}
  self.postMessage({ type: 'status', message: 'Using WASM backend' });
  return 'wasm';
}

self.onmessage = async (e) => {
  const { type } = e.data;

  if (type === 'configure') { Object.assign(config, e.data.config || {}); return; }

  if (type === 'init') {
    if (isInitializing || transcriber) return;
    isInitializing = true;
    try {
      let device = config.device === 'auto' ? await detectBestDevice() : config.device;
      const modelName = config.model.split('/').pop();
      const dtypeLabel = config.dtype === 'fp32' ? 'full precision' : config.dtype;
      self.postMessage({ type: 'status', message: \`Downloading \${modelName} (\${dtypeLabel}, \${device})...\` });
      self.postMessage({ type: 'progress', value: 0 });
      const pipelineOpts = {
        dtype: config.dtype,
        device: device,
        progress_callback: (progress) => {
          if (progress.status === 'downloading') {
            const pct = progress.total > 0 ? Math.round((progress.loaded / progress.total) * 100) : 0;
            self.postMessage({ type: 'progress', value: pct, file: progress.file });
          } else if (progress.status === 'loading') {
            self.postMessage({ type: 'status', message: 'Loading model weights...' });
          }
        }
      };
      try {
        transcriber = await pipeline('automatic-speech-recognition', config.model, pipelineOpts);
      } catch (firstErr) {
        if (device === 'webgpu') {
          self.postMessage({ type: 'status', message: \`WebGPU failed (\${firstErr.message}), falling back to WASM...\` });
          device = 'wasm'; pipelineOpts.device = 'wasm';
          transcriber = await pipeline('automatic-speech-recognition', config.model, pipelineOpts);
        } else { throw firstErr; }
      }
      config.device = device;
      self.postMessage({ type: 'progress', value: 100 });
      self.postMessage({ type: 'ready' });
      self.postMessage({ type: 'status', message: \`✓ \${modelName} loaded (\${dtypeLabel}, \${device}) — transcription active\` });
    } catch (err) {
      self.postMessage({ type: 'error', message: \`Model load failed: \${err.message}\` });
    } finally { isInitializing = false; }
  }

  if (type === 'transcribe') {
    const { chunk, audio } = e.data;
    if (!transcriber) { self.postMessage({ type: 'decoded', chunk, text: '' }); return; }
    try {
      const isEnglishOnly = config.model.includes('.en') || config.model.includes('moonshine');
      const opts = { return_timestamps: false };
      if (!isEnglishOnly) opts.language = config.language;
      const result = await transcriber(audio, opts);
      const text = (result && result.text) ? result.text : '';
      self.postMessage({ type: 'decoded', chunk, text });
    } catch (err) {
      self.postMessage({ type: 'error', message: \`Transcription error: \${err.message}\` });
      self.postMessage({ type: 'decoded', chunk, text: '' });
    }
  }

  if (type === 'terminate') { transcriber = null; self.close(); }
};
`;
