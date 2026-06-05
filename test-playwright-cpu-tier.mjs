// I5 gate 3 — Firefox + WebKit CPU-tier row (PRD R1).
// Drives Nemotron (the CPU/WASM engine) in Firefox and WebKit via Playwright:
//   crossOriginIsolated under COOP/COEP → load local-pinned model (no HF egress)
//   → feed the golden WAV via the real feed() call site → assert a transcript.
//
// The notetaker-server (port 8099) sends COOP same-origin + COEP require-corp
// (switched from credentialless 2026-06-05 per docs/research/spike-coep.md — the
// only COEP value WebKit/Safari honors for cross-origin isolation), so
// crossOriginIsolated must be true and threaded WASM must work in each engine,
// INCLUDING WebKit (this is the R1 Safari blocker resolution).
//
// Run: node test-playwright-cpu-tier.mjs <firefox|webkit>

import { firefox, webkit } from 'playwright';

const which = process.argv[2] || 'firefox';
const engineLib = which === 'webkit' ? webkit : firefox;
const URL = 'http://localhost:8099/index.html';

const GOLDEN = './crates/nemotron-asr/test-assets/test_16k.wav';

function decodeWavToF32(buf) {
  const dv = new DataView(buf);
  let off = 12, dataOff = 0, dataLen = 0;
  while (off < dv.byteLength) {
    const id = String.fromCharCode(dv.getUint8(off), dv.getUint8(off + 1), dv.getUint8(off + 2), dv.getUint8(off + 3));
    const sz = dv.getUint32(off + 4, true);
    if (id === 'data') { dataOff = off + 8; dataLen = sz; break; }
    off += 8 + sz;
  }
  const n = dataLen / 2; const f32 = new Float32Array(n);
  for (let i = 0; i < n; i++) f32[i] = dv.getInt16(dataOff + i * 2, true) / 32768;
  return f32;
}

(async () => {
  const browser = await engineLib.launch({ headless: true });
  const context = await browser.newContext();
  const page = await context.newPage();
  const consoleErrors = [];
  page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text()); });
  page.on('pageerror', (e) => consoleErrors.push('PAGEERROR: ' + e.message));

  const result = { browser: which, ua: null, coi: null, hwc: null, hasWebGPU: null,
                   loaded: false, transcript: null, ttft_s: null, rtf: null, err: null, consoleErrors: [] };
  try {
    await page.goto(URL, { waitUntil: 'domcontentloaded', timeout: 60000 });
    const env = await page.evaluate(() => ({
      ua: navigator.userAgent, coi: self.crossOriginIsolated,
      hwc: navigator.hardwareConcurrency, hasWebGPU: !!navigator.gpu,
      sab: typeof SharedArrayBuffer !== 'undefined',
    }));
    Object.assign(result, env);
    result.sharedArrayBuffer = env.sab;

    // Decode the golden WAV in Node, hand the array to the page.
    const fs = await import('fs');
    const wavBuf = fs.readFileSync(GOLDEN);
    const f32 = decodeWavToF32(wavBuf.buffer.slice(wavBuf.byteOffset, wavBuf.byteOffset + wavBuf.byteLength));
    const arr = Array.from(f32);

    // Load Nemotron + feed the golden WAV via the real feed() path. 5 min budget
    // (881MB encoder over localhost + WASM session build on a cold engine).
    const out = await page.evaluate(async (samples) => {
      const f = Float32Array.from(samples);
      const { NemotronEngine } = await import('./nemotron-engine.js');
      const eng = new NemotronEngine();
      let text = ''; let firstAt = null; let t0 = null;
      eng.onText = (frag) => { if (firstAt === null && frag && frag.trim()) firstAt = performance.now(); text += frag; };
      eng.onStatus = () => {};
      await eng.load();
      eng.reset();
      const audioSec = f.length / 16000;
      t0 = performance.now();
      const FEED = 4000;
      for (let i = 0; i < f.length; i += FEED) eng.feed(f.subarray(i, Math.min(i + FEED, f.length)));
      if (eng.finalize) await eng.finalize();
      await new Promise((r) => setTimeout(r, 400));
      const wallSec = (performance.now() - t0) / 1000;
      const s = eng.stats ? eng.stats() : {};
      return { text: text.trim(), ttft_s: firstAt ? (firstAt - t0) / 1000 : null,
               rtf: s.rtf != null ? s.rtf : +(wallSec / audioSec).toFixed(3),
               perfTtftMs: s.timeToFirstTextMs, chunks: s.chunks };
    }, arr);

    result.loaded = true;
    result.transcript = out.text;
    result.ttft_s = out.ttft_s;
    result.rtf = out.rtf;
    result.perfTtftMs = out.perfTtftMs;
    result.chunks = out.chunks;
  } catch (e) {
    result.err = e.message || String(e);
  } finally {
    result.consoleErrors = consoleErrors;
    await browser.close();
  }
  console.log('I5_RESULT ' + JSON.stringify(result));
})();
