/**
 * audio-spool.js — temporary OPFS executor for bounded Nemotron catch-up.
 *
 * The queue law lives in Rust (`silent_inference::nemotron_backlog`):
 * capacity, spill size, ordering, disk budget, and every overload decision.
 * This module is intentionally only the browser "hands" for typed commands:
 * write one Float32 segment, read one exact range, delete the completed file.
 *
 * Raw meeting audio reaches OPFS only when Nemotron is falling far enough
 * behind to cross its in-memory target. Each file is deleted immediately after
 * its last decoded range, and `clear()` removes every file owned by the current
 * engine session. No network operation exists in this module.
 */

const OPFS_OK = typeof navigator !== 'undefined'
  && navigator.storage
  && typeof navigator.storage.getDirectory === 'function';

const FILE_PREFIX = 'silent-nemotron-spool-';
const STALE_AFTER_MS = 24 * 60 * 60 * 1000;

function sessionId() {
  const random = typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : Math.random().toString(36).slice(2);
  return `${Date.now()}-${random}`;
}

/** Browser-only temporary file executor. Ordering never lives here. */
export class OpfsAudioSpool {
  constructor() {
    this.session = sessionId();
    this.files = new Map();
    this._root = null;
    // Crash leftovers are private local data, so clean old sessions
    // opportunistically. The 24 h age guard avoids touching another live tab.
    this._cleanup = this.cleanupStale().catch((error) => {
      console.warn('[nemotron-spool] stale cleanup failed:', error && error.message || error);
    });
  }

  get available() { return OPFS_OK; }

  async root() {
    if (!OPFS_OK) throw new Error('OPFS unavailable for bounded audio spill');
    if (!this._root) this._root = await navigator.storage.getDirectory();
    return this._root;
  }

  name(id) {
    if (!Number.isInteger(id) || id <= 0) throw new Error(`invalid spill id ${id}`);
    return `${FILE_PREFIX}${this.session}-${id}.f32`;
  }

  /** Persist one Rust-commanded Float32 segment and close it before returning. */
  async write(id, samples) {
    if (!(samples instanceof Float32Array)) {
      throw new TypeError('audio spill expects Float32Array samples');
    }
    const root = await this.root();
    const name = this.name(id);
    const handle = await root.getFileHandle(name, { create: true });
    const writable = await handle.createWritable();
    let committed = false;
    try {
      const bytes = new Uint8Array(samples.buffer, samples.byteOffset, samples.byteLength);
      await writable.write(bytes);
      await writable.close();
      committed = true;
    } catch (error) {
      try { await writable.abort(); } catch (_) {}
      try { await root.removeEntry(name); } catch (_) {}
      throw error;
    }
    if (!committed) throw new Error(`spill ${id} did not commit`);
    const file = await handle.getFile();
    if (file.size !== samples.byteLength) {
      try { await root.removeEntry(name); } catch (_) {}
      throw new Error(`spill ${id} size mismatch: ${file.size} != ${samples.byteLength}`);
    }
    this.files.set(id, { name, handle, samples: samples.length });
  }

  /** Read one exact Rust-commanded range without loading the whole spill file. */
  async read(id, offsetSamples, count) {
    const entry = this.files.get(id);
    if (!entry) throw new Error(`spill ${id} is not ready`);
    if (!Number.isInteger(offsetSamples) || !Number.isInteger(count)
        || offsetSamples < 0 || count <= 0
        || offsetSamples + count > entry.samples) {
      throw new RangeError(`invalid spill ${id} range ${offsetSamples}+${count}/${entry.samples}`);
    }
    const file = await entry.handle.getFile();
    const start = offsetSamples * Float32Array.BYTES_PER_ELEMENT;
    const end = start + count * Float32Array.BYTES_PER_ELEMENT;
    const bytes = await file.slice(start, end).arrayBuffer();
    if (bytes.byteLength !== count * Float32Array.BYTES_PER_ELEMENT) {
      throw new Error(`short read from spill ${id}: ${bytes.byteLength} bytes`);
    }
    return new Float32Array(bytes);
  }

  /** Delete a fully acknowledged spill file. */
  async remove(id) {
    const entry = this.files.get(id);
    if (!entry) return;
    this.files.delete(id);
    const root = await this.root();
    await root.removeEntry(entry.name);
  }

  /** Delete every temporary file owned by this engine session. */
  async clear() {
    const root = await this.root();
    const entries = Array.from(this.files.values());
    this.files.clear();
    await Promise.all(entries.map(async ({ name }) => {
      try { await root.removeEntry(name); } catch (error) {
        if (!/not found/i.test(String(error && error.message || error))) throw error;
      }
    }));
  }

  /** Remove crash leftovers older than a day, never files from another live tab. */
  async cleanupStale(now = Date.now()) {
    if (!OPFS_OK) return 0;
    const root = await this.root();
    let removed = 0;
    for await (const [name, entry] of root.entries()) {
      if (entry.kind !== 'file' || !name.startsWith(FILE_PREFIX) || !name.endsWith('.f32')) continue;
      const timestamp = Number(name.slice(FILE_PREFIX.length).split('-', 1)[0]);
      if (!Number.isFinite(timestamp) || now - timestamp < STALE_AFTER_MS) continue;
      try {
        await root.removeEntry(name);
        removed++;
      } catch (_) {}
    }
    if (removed) console.log(`[nemotron-spool] removed ${removed} stale temporary file(s)`);
    return removed;
  }
}

export default OpfsAudioSpool;
