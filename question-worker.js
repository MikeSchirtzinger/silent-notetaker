/**
 * question-worker.js
 * Web Worker: generates "smart questions" with Qwen3-0.6B via Transformers.js.
 *
 * Runs in its OWN thread so the ~6s WASM generation never blocks the main thread
 * (Voxtral's generate loop + the UI). WASM only on the live path — never WebGPU —
 * so it can't contend with Voxtral's WebGPU stream. Multithreaded WASM (needs the
 * page served cross-origin-isolated; see notetaker-server / coi-server.py).
 *
 * Protocol (postMessage):
 *   main → worker : {type:'configure', config}          set device/dtype/threads/model
 *                   {type:'load'}                        warm the model
 *                   {type:'generate', id, transcript, opts:{system?, maxTokens?}}
 *                   {type:'terminate'}
 *   worker → main : {type:'status', message}
 *                   {type:'progress', value, file}
 *                   {type:'ready', isolated, threads}
 *                   {type:'result', id, question, ms}
 *                   {type:'error', id?, message}
 */

import { pipeline, env } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@4.0.0-next.7/dist/transformers.min.js';

env.allowLocalModels = false;
env.useBrowserCache = true;

let generator = null;
let loading = null;
let chain = Promise.resolve(); // serialize generations so they never overlap on one session

let config = {
  model: 'onnx-community/Qwen3-0.6B-ONNX',
  device: 'wasm',  // live path is WASM-only (no Voxtral GPU contention)
  dtype: 'q4',     // q4 is fastest on CPU; q4f16 is WebGPU-oriented (fp16 emulated on WASM)
  threads: 4,      // sweet spot on a 10-core box: ~6.4s/question, leaves cores for Voxtral
  maxTokens: 64,
};

// One sharp question, from the USER's perspective, ready to ask out loud right now.
const DEFAULT_SYSTEM =
`You help the user think of ONE sharp, insightful question to ask out loud right now in this meeting.
The question must be from the user's perspective, specific to what was just discussed, and surface a gap, risk, or hidden assumption.
Output ONLY the question — one line, no preamble, no quotes.`;

async function load() {
  if (generator) return;
  if (loading) return loading;
  loading = (async () => {
    env.backends.onnx.wasm.numThreads = config.threads;
    self.postMessage({ type: 'status', message: `Loading ${config.model.split('/').pop()} (${config.dtype}, ${config.device}×${config.threads})…` });
    const progress_callback = (p) => {
      if (p.status === 'downloading' && p.total > 0) {
        self.postMessage({ type: 'progress', value: Math.round((p.loaded / p.total) * 100), file: p.file });
      }
    };
    try {
      generator = await pipeline('text-generation', config.model, { device: config.device, dtype: config.dtype, progress_callback });
    } catch (err) {
      // Don't fall back in here — the main thread owns the policy (e.g. a big model that
      // OOMs on WebGPU should demote to the smaller model, NOT crawl on WASM, and a
      // failed download shouldn't trigger a second multi-GB download). Report and bail.
      loading = null;
      self.postMessage({ type: 'loadfailed', model: config.model, device: config.device, message: String(err && err.message || err) });
      throw err;
    }
    self.postMessage({ type: 'ready', isolated: self.crossOriginIsolated, threads: env.backends.onnx.wasm.numThreads, device: config.device });
  })();
  return loading;
}

async function generateOne(id, transcript, opts = {}) {
  await load();
  // Keep questions natural-spoken: the transcript is NOT shared with other participants,
  // so the model must never reference "the transcript"/notes/recording in its question.
  const NO_META = ' Phrase the question as something natural to say out loud to the other people in the conversation. Never mention "the transcript", "the notes", "the meeting notes", or that anything is being recorded or transcribed.';
  const messages = [
    { role: 'system', content: (opts.system || DEFAULT_SYSTEM) + NO_META },
    { role: 'user', content: `Conversation so far:\n${transcript}` },
  ];
  // Qwen3 defaults to THINKING mode — must disable or it emits hundreds of <think> tokens.
  const prompt = generator.tokenizer.apply_chat_template(messages, {
    tokenize: false, add_generation_prompt: true, enable_thinking: false,
  });
  const t0 = performance.now();
  const out = await generator(prompt, {
    max_new_tokens: opts.maxTokens || config.maxTokens,
    do_sample: false,
    return_full_text: false,
  });
  const raw = Array.isArray(out) ? out[0].generated_text : out.generated_text;
  // Defensive: strip any stray <think>…</think> and surrounding quotes.
  const question = String(raw).replace(/<think>[\s\S]*?<\/think>/g, '').replace(/^["'\s]+|["'\s]+$/g, '').trim();
  self.postMessage({ type: 'result', id, question, ms: Math.round(performance.now() - t0) });
}

self.onmessage = (e) => {
  const { type } = e.data;
  if (type === 'configure') { Object.assign(config, e.data.config || {}); return; }
  if (type === 'load') { load().catch((err) => self.postMessage({ type: 'error', message: String(err && err.message || err) })); return; }
  if (type === 'generate') {
    const { id, transcript, opts } = e.data;
    const run = () => generateOne(id, transcript, opts);
    chain = chain.then(run, run).catch((err) => self.postMessage({ type: 'error', id, message: String(err && err.message || err) }));
    return;
  }
  if (type === 'terminate') { generator = null; self.close(); }
};
