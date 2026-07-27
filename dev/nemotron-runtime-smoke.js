/**
 * Real-browser regression probe for the two Nemotron browser boundaries:
 * Rust/WASM backlog policy ↔ temporary OPFS executor, and streaming Response ↔
 * committed model cache. It uses real OPFS and the built silent-web Wasm.
 */

import init, { WasmNemotronBacklog } from '../crates/silent-web/pkg/silent_web.js';
import { NemotronEngine } from '../nemotron-engine.js';
import { OpfsAudioSpool } from '../apps/web/js/audio-spool.js';
import {
  modelCacheAvailable,
  readModel,
  removeModel,
  writeModelResponse,
} from '../apps/web/js/model-cache.js';

function check(condition, message) {
  if (!condition) throw new Error(message);
}

function parsed(value) { return JSON.parse(value); }

async function backlogSmoke() {
  await init();
  const backlog = new WasmNemotronBacklog(4_000);
  const spool = new OpfsAudioSpool();
  const totalSamples = 41 * 16_000; // crosses the 40 s spill high-water mark once
  let cursor = 0;

  while (cursor < totalSamples) {
    const count = Math.min(4_000, totalSamples - cursor);
    const batch = new Float32Array(count);
    for (let i = 0; i < count; i++) batch[i] = (cursor + i) % 10_000;
    backlog.pushSamples(batch);
    cursor += count;

    for (;;) {
      const desc = parsed(backlog.stagedSpill());
      if (!desc) break;
      const samples = backlog.takeStagedSpill(desc.id);
      await spool.write(desc.id, samples);
      backlog.markSpillReady(desc.id);
    }
  }

  const afterPush = parsed(backlog.snapshot());
  check(afterPush.pending_samples === totalSamples, 'backlog lost samples during push');
  check(afterPush.spooled_samples === 10 * 16_000, 'expected one 10 s spill');
  check(afterPush.resident_samples <= 60 * 16_000, 'resident audio exceeded 60 s bound');

  let decodedSpill = 0;
  for (;;) {
    const action = parsed(backlog.nextDecode(false));
    if (action.source !== 'spool') break;
    const samples = await spool.read(action.id, action.offset_samples, action.count);
    for (let i = 0; i < samples.length; i++) {
      check(samples[i] === (decodedSpill + i) % 10_000, 'spilled PCM order mismatch');
    }
    decodedSpill += samples.length;
    if (backlog.ackSpill(action.id, action.count)) await spool.remove(action.id);
  }
  check(decodedSpill === 10 * 16_000, 'did not drain the complete spill');

  const memoryAction = parsed(backlog.nextDecode(false));
  check(memoryAction.source === 'memory', 'memory must follow older spill audio');
  const memory = backlog.takeMemory(memoryAction.count);
  check(memory[0] === decodedSpill % 10_000, 'memory resumed at the wrong sample');
  backlog.ackMemory(memoryAction.count);

  const finalSnapshot = parsed(backlog.snapshot());
  backlog.reset();
  await spool.clear();
  return { afterPush, finalSnapshot, decodedSpill };
}

async function cacheSmoke() {
  check(modelCacheAvailable(), 'OPFS model cache unavailable');
  const key = `smoke/model-cache/${Date.now()}`;
  const total = 4 * 1024 * 1024;
  const chunkSize = 64 * 1024;
  let offset = 0;
  const progress = [];
  const stream = new ReadableStream({
    pull(controller) {
      if (offset >= total) {
        controller.close();
        return;
      }
      const count = Math.min(chunkSize, total - offset);
      const chunk = new Uint8Array(count);
      for (let i = 0; i < count; i++) chunk[i] = (offset + i) % 251;
      offset += count;
      controller.enqueue(chunk);
    },
  });
  const response = new Response(stream, {
    status: 200,
    headers: {
      'content-type': 'application/octet-stream',
      'content-length': String(total),
    },
  });

  try {
    const bytes = await writeModelResponse(
      key,
      response,
      { url: 'smoke://streamed-response' },
      (loaded, expected) => progress.push([loaded, expected]),
    );
    check(bytes.length === total, 'committed cache length mismatch');
    for (let i = 0; i < bytes.length; i += chunkSize) {
      check(bytes[i] === i % 251, `cache content mismatch at ${i}`);
    }
    check(progress.length === total / chunkSize, 'progress did not follow stream chunks');
    check(progress.at(-1)[0] === total && progress.at(-1)[1] === total,
      'progress did not reach exact content length');
    return { bytes: bytes.length, progressEvents: progress.length };
  } finally {
    await removeModel(key);
  }
}

async function shortCacheSmoke() {
  check(modelCacheAvailable(), 'OPFS model cache unavailable');
  const key = `smoke/model-cache-short/${Date.now()}`;
  const actual = 128 * 1024;
  const declared = actual * 2;
  const response = new Response(new Uint8Array(actual), {
    status: 200,
    headers: {
      'content-type': 'application/octet-stream',
      'content-length': String(declared),
    },
  });

  let message = '';
  try {
    await writeModelResponse(key, response, { url: 'smoke://short-response' });
  } catch (error) {
    message = error && error.message || String(error);
  }
  check(message.includes('short model download'), 'short response did not fail loudly');
  check(await readModel(key) === null, 'short response left a readable cache entry');
  await removeModel(key);
  return { rejected: true, cacheEntryRemoved: true };
}

async function fatalCleanupSmoke() {
  const engine = new NemotronEngine();
  const spool = new OpfsAudioSpool();
  engine._spool = spool;
  const id = 1;
  const name = spool.name(id);
  await spool.write(id, new Float32Array(4_000));
  let callbackMessage = '';
  engine.onFatal = (error) => { callbackMessage = error.message; };
  engine._fail(new Error('runtime smoke hard stop'));

  let exists = true;
  for (let attempt = 0; attempt < 50 && exists; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
    try {
      await (await spool.root()).getFileHandle(name);
    } catch (_) {
      exists = false;
    }
  }
  check(callbackMessage === 'runtime smoke hard stop', 'fatal callback did not receive the error');
  check(!exists, 'fatal stop left temporary PCM in OPFS');
  return { callbackFired: true, temporaryPcmRemoved: true };
}

export async function runNemotronRuntimeSmoke() {
  const started = performance.now();
  const result = {
    crossOriginIsolated: self.crossOriginIsolated,
    backlog: await backlogSmoke(),
    cache: await cacheSmoke(),
    shortCache: await shortCacheSmoke(),
    fatalCleanup: await fatalCleanupSmoke(),
  };
  result.elapsedMs = Math.round(performance.now() - started);
  window.__nemotronRuntimeSmoke = result;
  return result;
}
