/**
 * model-cache.js — on-device cache for large model weights (Tier 1: silent OPFS).
 *
 * Why this exists: the big ASR weights (Nemotron encoder ~881 MB) were fetched
 * into memory on every load and only ever rode the browser's HTTP disk cache —
 * the flakiest cache tier, which Chrome routinely refuses to retain for single
 * ~GB responses or evicts first under disk pressure. Result: a multi-minute
 * re-download on many repeat visits, even though "cached after first load" was
 * the promise. This module gives the weights a durable, app-controlled home.
 *
 * PRIVACY — this changes NOTHING about the trust boundary:
 *   - OPFS (Origin Private File System) is the browser's per-site sandbox
 *     storage. It is on-device, origin-private, and unreadable by any other
 *     site. It is the SAME privacy posture as the Cache API / IndexedDB the app
 *     already uses. There is NO permission prompt — it is silent, like the
 *     existing model cache for the transformers.js engines (useBrowserCache).
 *   - We store ONLY public model weight files (the AI itself). Meeting audio and
 *     notes never touch this — those stay in their existing IndexedDB store.
 *   - Nothing is uploaded. OPFS makes no network requests. This module only
 *     reads/writes local files; it cannot egress.
 *
 * navigator.storage.persist() is requested before the first write so the cache
 * becomes durable (exempt from automatic eviction). On Chrome/Edge persist() is
 * decided by engagement heuristics and shows NO dialog (it just returns a
 * boolean). On Firefox it may prompt; on Safari it resolves silently.
 *
 * Atomicity: each model is stored as <name>.bin + a <name>.meta sidecar.
 * createWritable() commits only on close(), so an interrupted write never
 * commits a partial .bin; we write the data file first and the meta sidecar
 * second, so a present .meta always implies a complete .bin, and read verifies
 * the byte length against meta.len before trusting the bytes.
 */

const OPFS_OK = typeof navigator !== 'undefined'
  && navigator.storage && typeof navigator.storage.getDirectory === 'function';

export function modelCacheAvailable() { return OPFS_OK; }

let _root = null;
async function root() {
  if (_root) return _root;
  if (!OPFS_OK) throw new Error('OPFS unavailable in this browser');
  _root = await navigator.storage.getDirectory();
  return _root;
}

// Flatten a logical key ("nemotron/<rev>/encoder.onnx") into a safe flat filename.
function safe(key) { return String(key).replace(/[^a-z0-9_.-]+/gi, '_'); }

async function handle(name, create) {
  return (await root()).getFileHandle(name, { create });
}

// ── persistent storage (memoized; silent on Chrome/Edge — no prompt) ──────────
let _persistP = null;
export function requestPersistentStorage() {
  if (_persistP) return _persistP;
  _persistP = (async () => {
    try {
      if (!(navigator && navigator.storage && navigator.storage.persist)) return false;
      if (navigator.storage.persisted && await navigator.storage.persisted()) return true;
      const ok = await navigator.storage.persist();
      console.log(`[model-cache] persistent storage ${ok ? 'GRANTED (durable)' : 'best-effort (evictable)'}`);
      return ok;
    } catch (e) {
      console.warn('[model-cache] persist() failed:', (e && e.message) || e);
      return false;
    }
  })();
  return _persistP;
}

/**
 * Read a cached model by logical key. Returns a Uint8Array on a verified hit, or
 * null on a miss / unavailable OPFS / any error (caller falls back to network).
 */
export async function readModel(key) {
  if (!OPFS_OK) return null;
  const base = safe(key);
  let metaH, dataH;
  try {
    metaH = await handle(base + '.meta', false);   // throws if absent → miss
    dataH = await handle(base + '.bin', false);
  } catch {
    return null;   // not cached
  }
  try {
    const meta = JSON.parse(await (await metaH.getFile()).text());
    const file = await dataH.getFile();
    if (!meta || file.size !== meta.len) {
      console.warn(`[model-cache] ${key}: size mismatch (have ${file.size}, expected ${meta && meta.len}) — ignoring cache`);
      return null;
    }
    return new Uint8Array(await file.arrayBuffer());
  } catch (e) {
    console.warn(`[model-cache] read ${key} failed:`, (e && e.message) || e);
    return null;
  }
}

/**
 * Write-through a model by logical key. Best-effort: requests durable storage,
 * writes the data file atomically, then the meta sidecar. Throws on failure so
 * the caller can log + continue (the network bytes are already in hand).
 */
export async function writeModel(key, bytes, info = {}) {
  if (!OPFS_OK) throw new Error('OPFS unavailable');
  await requestPersistentStorage();   // make the cache durable before we fill it
  const base = safe(key);

  const dataH = await handle(base + '.bin', true);
  const dw = await dataH.createWritable();
  try { await dw.write(bytes); } finally { await dw.close(); }   // commits on close

  const metaH = await handle(base + '.meta', true);
  const mw = await metaH.createWritable();
  try {
    await mw.write(JSON.stringify({ len: bytes.length, url: info.url || null, ts: Date.now() }));
  } finally { await mw.close(); }
}

/**
 * Stream a network Response directly into OPFS, commit its metadata, then read
 * the completed file once for model construction.
 *
 * This is the cold-load path for large weights. It deliberately never builds a
 * full network Uint8Array while an OPFS write and WASM/ONNX session build are
 * also active. At most one response chunk is held during download; the method
 * resolves only after the data file and sidecar are closed and verified.
 *
 * `onProgress(loaded, total)` is called after each committed stream chunk.
 */
export async function writeModelResponse(key, response, info = {}, onProgress = null) {
  if (!OPFS_OK) throw new Error('OPFS unavailable');
  if (!response || !response.ok) {
    throw new Error(`cannot cache unsuccessful response (${response && response.status})`);
  }
  await requestPersistentStorage();
  const base = safe(key);
  const total = +(response.headers.get('content-length') || 0);
  const dataH = await handle(base + '.bin', true);
  const writable = await dataH.createWritable();
  let loaded = 0;
  try {
    if (response.body && typeof response.body.getReader === 'function') {
      const reader = response.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        await writable.write(value);
        loaded += value.byteLength;
        if (onProgress) onProgress(loaded, total);
      }
    } else {
      const bytes = new Uint8Array(await response.arrayBuffer());
      await writable.write(bytes);
      loaded = bytes.byteLength;
      if (onProgress) onProgress(loaded, total || loaded);
    }
    await writable.close();
  } catch (error) {
    try { await writable.abort(); } catch (_) {}
    throw error;
  }

  if (total && loaded !== total) {
    await removeModel(key);
    throw new Error(`short model download for ${key}: ${loaded} != ${total} bytes`);
  }

  const metaH = await handle(base + '.meta', true);
  const meta = JSON.stringify({ len: loaded, url: info.url || null, ts: Date.now() });
  const mw = await metaH.createWritable();
  try {
    await mw.write(meta);
    await mw.close();
  } catch (error) {
    try { await mw.abort(); } catch (_) {}
    await removeModel(key);
    throw error;
  }

  const bytes = await readModel(key);
  if (!bytes || bytes.length !== loaded) {
    await removeModel(key);
    throw new Error(`committed model cache verification failed for ${key}`);
  }
  return bytes;
}

// ── management helpers (debug + a future "Manage models" panel) ───────────────

/** Delete one logical model entry. Returns the number of files removed. */
export async function removeModel(key) {
  if (!OPFS_OK) return 0;
  const base = safe(key);
  const r = await root();
  let removed = 0;
  for (const suffix of ['.bin', '.meta']) {
    try {
      await r.removeEntry(base + suffix);
      removed++;
    } catch (_) {}
  }
  return removed;
}

/** Delete every file this cache created. Returns the count removed. */
export async function clearModelCache() {
  if (!OPFS_OK) return 0;
  const r = await root();
  let n = 0;
  for await (const [name, entry] of r.entries()) {
    if (entry.kind === 'file' && (name.endsWith('.bin') || name.endsWith('.meta'))) {
      try { await r.removeEntry(name); n++; } catch (e) { console.warn('[model-cache] remove', name, e); }
    }
  }
  console.log(`[model-cache] cleared ${n} cached file(s)`);
  return n;
}

/** Summary for a management UI / quick console check: per-file sizes + quota. */
export async function cacheStats() {
  const out = { files: [], usage: null, quota: null, persisted: null };
  try {
    if (navigator.storage && navigator.storage.estimate) {
      const est = await navigator.storage.estimate();
      out.usage = est.usage; out.quota = est.quota;
    }
    if (navigator.storage && navigator.storage.persisted) out.persisted = await navigator.storage.persisted();
    if (OPFS_OK) {
      const r = await root();
      for await (const [name, entry] of r.entries()) {
        if (entry.kind === 'file' && name.endsWith('.bin')) {
          out.files.push({ name, size: (await entry.getFile()).size });
        }
      }
    }
  } catch (e) { console.warn('[model-cache] stats failed:', (e && e.message) || e); }
  return out;
}

// Small debug handle so you can inspect/clear the cache from the console while
// testing: __modelCache.stats() / __modelCache.clear().
if (typeof window !== 'undefined') {
  window.__modelCache = Object.assign(window.__modelCache || {}, {
    stats: cacheStats, clear: clearModelCache, readModel, writeModel,
    writeModelResponse, removeModel, modelCacheAvailable, requestPersistentStorage,
  });
}
