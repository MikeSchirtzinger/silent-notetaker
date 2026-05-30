/**
 * transcription-worker.js
 * Web Worker for Moonshine/Whisper ONNX model inference via Transformers.js
 * Runs in a separate thread to avoid blocking the main UI
 *
 * Accuracy improvements over v1:
 *   - Configurable model and quantization (default: whisper-small, fp32)
 *   - Longer chunks (5s) with 1s overlap for boundary continuity
 *   - Energy-based VAD to skip silence (reduces hallucinations)
 *   - WebGPU with WASM fallback for faster inference on larger models
 */

import { pipeline, env } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3/dist/transformers.min.js';

env.allowLocalModels = false;
env.useBrowserCache = true;

let transcriber = null;
let isInitializing = false;
let pendingAudioChunks = [];
let isProcessing = false;

// Configurable — main thread can override via 'configure' message
let config = {
  model: 'onnx-community/whisper-large-v3-turbo', // Best accuracy available in browser (~7.75% WER)
  dtype: 'q4f16',                                 // q4f16 mixed quantization — best for WebGPU
  device: 'webgpu',                               // GPU acceleration required for this model size
  chunkSeconds: 5,                              // Longer chunks = more context
  vadThreshold: 0.008,                          // RMS energy below this = silence (skip)
  language: 'en',
  corrections: {},                              // { "wrong word": "right word" }
};

/**
 * Simple energy-based Voice Activity Detection
 * Returns true if the audio chunk contains speech (above energy threshold)
 */
function hasSpeech(audio, threshold) {
  let sumSquares = 0;
  // Sample every 4th value for speed
  const step = 4;
  let count = 0;
  for (let i = 0; i < audio.length; i += step) {
    sumSquares += audio[i] * audio[i];
    count++;
  }
  const rms = Math.sqrt(sumSquares / count);
  return rms > threshold;
}

/**
 * Text-level deduplication: strip leading words that repeat from previous chunk's tail
 */
let prevTailWords = [];
const DEDUP_WINDOW = 12; // compare up to 12 words

function deduplicateText(text) {
  if (prevTailWords.length === 0) return text;

  const words = text.split(/\s+/);
  const prevTail = prevTailWords.join(' ').toLowerCase();

  // Find the longest prefix of `words` that appears at the end of prevTail
  let bestCut = 0;
  for (let len = 1; len <= Math.min(words.length, DEDUP_WINDOW); len++) {
    const candidate = words.slice(0, len).join(' ').toLowerCase();
    if (prevTail.endsWith(candidate) || prevTail.includes(candidate)) {
      bestCut = len;
    }
  }

  if (bestCut > 0) {
    return words.slice(bestCut).join(' ');
  }
  return text;
}

/**
 * Apply user-defined corrections dictionary
 */
function applyCorrections(text) {
  for (const [wrong, right] of Object.entries(config.corrections || {})) {
    const re = new RegExp(wrong.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
    text = text.replace(re, right);
  }
  return text;
}

/**
 * Process queued audio chunks sequentially
 */
async function processQueue() {
  if (isProcessing || pendingAudioChunks.length === 0 || !transcriber) return;
  isProcessing = true;

  while (pendingAudioChunks.length > 0) {
    const audio = pendingAudioChunks.shift();

    // VAD check — skip silence
    if (!hasSpeech(audio, config.vadThreshold)) {
      continue;
    }

    try {
      // English-only models (.en) reject language/task params
      const isEnglishOnly = config.model.includes('.en') || config.model.includes('moonshine');
      const opts = { return_timestamps: false };
      if (!isEnglishOnly) {
        opts.language = config.language;
      }
      const result = await transcriber(audio, opts);

      if (result && result.text) {
        let text = result.text.trim();
        if (text.length > 1 && !isHallucination(text)) {
          // Deduplicate overlap from previous chunk
          text = deduplicateText(text);
          // Apply corrections dictionary
          text = applyCorrections(text);

          if (text.trim().length > 0) {
            // Save tail for next chunk's dedup
            const words = text.split(/\s+/);
            prevTailWords = words.slice(-DEDUP_WINDOW);
            self.postMessage({ type: 'final', text: text.trim() });
          }
        }
      }
    } catch (err) {
      self.postMessage({ type: 'error', message: `Transcription error: ${err.message}` });
    }
  }

  isProcessing = false;
}

/**
 * Detect common Whisper hallucination patterns
 */
function isHallucination(text) {
  const lower = text.toLowerCase();
  const hallucinations = [
    'thank you for watching',
    'thanks for watching',
    'subscribe to my channel',
    'please like and subscribe',
    'thank you for listening',
    'thanks for listening',
    'you',  // Whisper often outputs just "You" on faint audio
    '...',
    'the end',
    'bye',
    'goodbye',
  ];
  // Exact match hallucinations
  if (hallucinations.includes(lower) || hallucinations.includes(lower.replace(/[.!?,]/g, ''))) {
    return true;
  }
  // Repeated phrase hallucination (same word 4+ times)
  const words = lower.split(/\s+/);
  if (words.length >= 4) {
    const unique = new Set(words);
    if (unique.size === 1) return true;
  }
  return false;
}

/**
 * Try WebGPU, fall back to WASM
 */
async function detectBestDevice() {
  try {
    if (typeof navigator !== 'undefined' && navigator.gpu) {
      const adapter = await navigator.gpu.requestAdapter();
      if (adapter) {
        self.postMessage({ type: 'status', message: 'WebGPU available — using GPU acceleration' });
        return 'webgpu';
      }
    }
  } catch (_) {}
  self.postMessage({ type: 'status', message: 'Using WASM backend' });
  return 'wasm';
}

// ── Message handler ──────────────────────────────────────

self.onmessage = async (e) => {
  const { type } = e.data;

  if (type === 'configure') {
    // Allow main thread to override settings before init
    Object.assign(config, e.data.config || {});
    return;
  }

  if (type === 'corrections') {
    // Update corrections dictionary mid-meeting
    config.corrections = e.data.corrections || {};
    return;
  }

  if (type === 'init') {
    if (isInitializing || transcriber) return;
    isInitializing = true;

    try {
      // Auto-detect best device if set to auto, otherwise use configured
      let device = config.device === 'auto' ? await detectBestDevice() : config.device;

      const modelName = config.model.split('/').pop();
      const dtypeLabel = config.dtype === 'fp32' ? 'full precision' : config.dtype;
      self.postMessage({
        type: 'status',
        message: `Downloading ${modelName} (${dtypeLabel}, ${device})...`
      });
      self.postMessage({ type: 'progress', value: 0 });

      const pipelineOpts = {
        dtype: config.dtype,
        device: device,
        progress_callback: (progress) => {
          if (progress.status === 'downloading') {
            const pct = progress.total > 0
              ? Math.round((progress.loaded / progress.total) * 100)
              : 0;
            self.postMessage({ type: 'progress', value: pct, file: progress.file });
          } else if (progress.status === 'loading') {
            self.postMessage({ type: 'status', message: 'Loading model weights...' });
          }
        }
      };

      try {
        transcriber = await pipeline('automatic-speech-recognition', config.model, pipelineOpts);
      } catch (firstErr) {
        // If WebGPU failed, fall back to WASM
        if (device === 'webgpu') {
          self.postMessage({
            type: 'status',
            message: `WebGPU failed (${firstErr.message}), falling back to WASM...`
          });
          device = 'wasm';
          pipelineOpts.device = 'wasm';
          transcriber = await pipeline('automatic-speech-recognition', config.model, pipelineOpts);
        } else {
          throw firstErr;
        }
      }

      config.device = device;
      self.postMessage({ type: 'progress', value: 100 });
      self.postMessage({ type: 'ready' });
      self.postMessage({
        type: 'status',
        message: `✓ ${modelName} loaded (${dtypeLabel}, ${device}) — transcription active`
      });

      if (pendingAudioChunks.length > 0) processQueue();

    } catch (err) {
      self.postMessage({ type: 'error', message: `Model load failed: ${err.message}` });
    } finally {
      isInitializing = false;
    }
  }

  if (type === 'transcribe') {
    const audio = e.data.audio;
    if (!transcriber) {
      pendingAudioChunks.push(audio);
      return;
    }
    pendingAudioChunks.push(audio);
    processQueue();
  }

  if (type === 'terminate') {
    transcriber = null;
    pendingAudioChunks = [];
    prevTailWords = [];
    self.close();
  }
};
