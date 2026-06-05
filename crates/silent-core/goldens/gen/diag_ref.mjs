// Reference crash-diagnostics ring buffer — a faithful, DOM-free port of the
// `Diag` sampler in index.html (~1899-2005) and the prior-trail surfacing on
// load (~6480-6504). This file is the BEHAVIOR CONTRACT for the Rust
// `silent_core::diag` port:
//
//   1. The localStorage row SHAPE and KEY ORDER must be byte-identical to what
//      `JSON.stringify(rows)` produces, so a Rust-written `notetakerDiag` value
//      and a JS-written one are interchangeable (prior-trail recovery across a
//      JS→Rust cutover, and `dumpDiag()` parsing either, both stay valid).
//   2. The numeric NORMALIZATION (`+(bytes/1048576).toFixed(1)`,
//      `+(deltaMs).toFixed(1)`) must reproduce `Number.prototype.toFixed(1)` +
//      `JSON.stringify` of the result EXACTLY — including the IEEE-754 tie
//      behavior (`0.05`->`0.1` but `0.15`->`0.1`, `12.35`->`12.3`) and the
//      whole-number elision (`12.0`->`12`).
//   3. The bounded ring (push, then `while(len>MAX) shift`) and the start()
//      semantics (clear prior trail, baseline row at t=0) must match.
//   4. The prior-trail summary line format must match (peak heap, last elapsed,
//      and the per-row `t+Ns heap=... ctxLen=... ...` line incl. the DEVICE-LOST
//      tag).
//
// Sourced verbatim from index.html:
//   - DIAG_KEY / DIAG_MAX_ROWS / DIAG_INTERVAL_MS           ~1910-1912
//   - the `c` counters                                       ~1918-1927
//   - `sample()` row object (KEY ORDER IS LOAD-BEARING)      ~1936-1962
//   - the bounded-ring write (`push` then `while > MAX shift`) ~1956-1959
//   - `start()` (clear trail, baseline sample)               ~1967-1977
//   - loop hooks onLoopIter/onRecycle/onPut/onDeviceLost     ~1982-1992
//   - prior-trail surfacing line format                      ~6489-6497
//
// The row carries `items`/`words` (DOM `.transcript-item`/`.transcript-word`
// counts) and `writeAbs` (the Voxtral ring write cursor). In the Rust core these
// are inputs SUPPLIED BY THE HOST each tick (the core has no DOM and no ring),
// so the fixture records them as explicit per-tick inputs, exactly as the wasm
// subscriber will pass them in. `iso` (the wall-clock ISO timestamp) and
// `elapsedSec` are likewise host-supplied (the Rust core has no clock, PRD R5);
// the fixture pins them as inputs so the row bytes are fully determined.
//
// Run: node diag_ref.mjs  -> writes ../diag/diag.json
//
// PURE of the DOM/clock: every value is an explicit input, so the fixtures are
// exactly reproducible and the Rust golden test (tests/diag_golden.rs) replays
// them and asserts byte-equality.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// =====================================================================
// Constants — verbatim from index.html.
// =====================================================================
const DIAG_KEY = "notetakerDiag";
const DIAG_MAX_ROWS = 200; // bounded ring — never grows unbounded
const DIAG_INTERVAL_MS = 3000; // ~3s cadence (informational; the host owns timer)

// =====================================================================
// The numeric normalizers — verbatim from index.html `sample()`.
// `heapXxxMB` is `+(bytes/1048576).toFixed(1)`; `lastStepMs` is the
// `+(now - lastPutAt).toFixed(1)` produced by onPut. Both are "round to one
// decimal, then collapse a trailing .0 to an integer" — i.e. exactly what
// `JSON.stringify(+x.toFixed(1))` yields.
// =====================================================================
const BYTES_PER_MB = 1048576;
function heapMb(bytes) {
  return +(bytes / BYTES_PER_MB).toFixed(1);
}
function oneDecimal(x) {
  return +x.toFixed(1);
}

// =====================================================================
// The row builder — KEY ORDER COPIED VERBATIM from index.html:1939-1955.
// `mem` is the `performance.memory` snapshot (or null when unavailable). All
// of `items/words/loopIter/recycle/ctxLen/genStepsTotal/inputTokens/
// lastStepMs/deviceLost/writeAbs` are supplied by the caller (the host) — the
// Rust core takes them as inputs. `iso`/`elapsedSec` are host clock values.
// =====================================================================
function buildRow(input) {
  const mem = input.mem; // { used, total, limit } in BYTES, or null
  return {
    iso: input.iso,
    elapsedSec: input.elapsedSec,
    heapUsedMB: mem ? heapMb(mem.used) : null,
    heapTotalMB: mem ? heapMb(mem.total) : null,
    heapLimitMB: mem ? heapMb(mem.limit) : null,
    items: input.items,
    words: input.words,
    loopIter: input.loopIter,
    recycle: input.recycle,
    ctxLen: input.ctxLen,
    genStepsTotal: input.genStepsTotal,
    inputTokens: input.inputTokens,
    lastStepMs: input.lastStepMs, // already a +(…).toFixed(1) number (or 0)
    deviceLost: input.deviceLost,
    writeAbs: input.writeAbs,
  };
}

// The bounded-ring write — verbatim from index.html:1956-1959.
function ringPush(rows, row, max) {
  rows.push(row);
  while (rows.length > max) rows.shift();
  return rows;
}

// =====================================================================
// Fixture 1 — normalization cases. Each is a raw f64 whose
// `JSON.stringify(+x.toFixed(1))` the Rust port must reproduce exactly,
// including the IEEE-754 tie cases. `outJson` is the literal serialized token.
// =====================================================================
const oneDecimalRaws = [
  0, 0.0, 0.04, 0.05, 0.15, 0.25, 0.35, 0.45, 0.55, 0.65, 0.75, 0.85, 0.95,
  1.0, 1.05, 1.15, 1.25, 2.5, 7.0, 7.1, 12.0, 12.04, 12.05, 12.15, 12.25,
  12.34, 12.35, 12.349, 12.45, 100.0, 1234.55, 1234.56, 1234.65, 99999.9,
  11.999999, 13.0, 150.0,
];
const oneDecimalCases = oneDecimalRaws.map((raw) => {
  const v = oneDecimal(raw);
  return { raw, value: v, outJson: JSON.stringify(v) };
});

// Heap byte->MB cases: the division happens in f64 then toFixed(1). These mirror
// real `performance.memory` byte magnitudes.
const heapByteCases = [
  0, 1048576, 12582912, 13631488, 104857600, 157286400, 536870912, 1073741824,
  2147483648, 1610612736, 123456789, 999999999, 3221225472,
].map((bytes) => {
  const v = heapMb(bytes);
  return { bytes, value: v, outJson: JSON.stringify(v) };
});

// =====================================================================
// Fixture 2 — full rows. A representative sequence of sampler ticks covering:
// memory present + absent (null heap), device-lost set, a growing ctxLen,
// recycle increments, fractional step-times, and a null writeAbs. `rowJson` is
// `JSON.stringify(row)` — the exact bytes one ring slot occupies. `arrayJson`
// is `JSON.stringify(rows)` after each push (the exact localStorage value).
// =====================================================================
const tickInputs = [
  // baseline at t=0, memory available, nothing happened yet
  {
    iso: "2026-06-04T17:00:00.000Z",
    elapsedSec: 0,
    mem: { used: 52428800, total: 67108864, limit: 4294967296 },
    items: 0, words: 0, loopIter: 0, recycle: 0, ctxLen: 0,
    genStepsTotal: 0, inputTokens: 0, lastStepMs: 0, deviceLost: "",
    writeAbs: null,
  },
  // 3s in: first generate context, some tokens, fractional step time
  {
    iso: "2026-06-04T17:00:03.000Z",
    elapsedSec: 3,
    mem: { used: 89128960, total: 134217728, limit: 4294967296 },
    items: 2, words: 14, loopIter: 1, recycle: 0, ctxLen: 37,
    genStepsTotal: 37, inputTokens: 8, lastStepMs: oneDecimal(41.27),
    deviceLost: "", writeAbs: 48000,
  },
  // 6s in: recycled once, heap ticked down (sawtooth), fractional step
  {
    iso: "2026-06-04T17:00:06.000Z",
    elapsedSec: 6,
    mem: { used: 71303168, total: 134217728, limit: 4294967296 },
    items: 5, words: 33, loopIter: 2, recycle: 1, ctxLen: 12,
    genStepsTotal: 95, inputTokens: 8, lastStepMs: oneDecimal(38.05),
    deviceLost: "", writeAbs: 96000,
  },
  // 9s in: performance.memory UNAVAILABLE (Firefox) -> null heap fields
  {
    iso: "2026-06-04T17:00:09.000Z",
    elapsedSec: 9,
    mem: null,
    items: 7, words: 51, loopIter: 3, recycle: 2, ctxLen: 5,
    genStepsTotal: 160, inputTokens: 8, lastStepMs: oneDecimal(45.5),
    deviceLost: "", writeAbs: 144000,
  },
  // 12s in: DEVICE-LOST observed (out-of-band sample triggered by onDeviceLost)
  {
    iso: "2026-06-04T17:00:12.000Z",
    elapsedSec: 12,
    mem: { used: 2105540608, total: 2147483648, limit: 2147483648 },
    items: 9, words: 70, loopIter: 4, recycle: 3, ctxLen: 110,
    genStepsTotal: 240, inputTokens: 8,
    lastStepMs: oneDecimal(512.94),
    deviceLost: "Device lost: out of memory while allocating buffer",
    writeAbs: 192000,
  },
];

let rows = [];
const rowFixtures = tickInputs.map((input) => {
  const row = buildRow(input);
  ringPush(rows, row, DIAG_MAX_ROWS);
  return {
    input,
    rowJson: JSON.stringify(row),
    arrayJson: JSON.stringify(rows),
  };
});

// =====================================================================
// Fixture 3 — bounded ring eviction. Push (MAX_ROWS + extra) rows; assert the
// ring keeps exactly the last MAX_ROWS, in order. We pin the final length, the
// first/last surviving `elapsedSec`, and the full `JSON.stringify(rows)` for a
// small MAX so the byte-shape of eviction is fixed without a 200-row blob.
// =====================================================================
function ringRun(max, count) {
  const r = [];
  for (let i = 0; i < count; i++) {
    const row = buildRow({
      iso: "2026-06-04T17:00:00.000Z",
      elapsedSec: i,
      mem: { used: (i + 1) * 1048576, total: 134217728, limit: 4294967296 },
      items: i, words: i * 7, loopIter: i, recycle: 0, ctxLen: i % 16,
      genStepsTotal: i * 4, inputTokens: 8, lastStepMs: 0, deviceLost: "",
      writeAbs: i * 48000,
    });
    ringPush(r, row, max);
  }
  return r;
}
// Small ring (max 4, push 7) -> exact serialized bytes after eviction.
const smallRing = ringRun(4, 7);
// Real ring (max 200, push 250) -> just the invariants (length, ends).
const bigRing = ringRun(DIAG_MAX_ROWS, 250);
const ringFixtures = {
  smallMax: 4,
  smallCount: 7,
  smallRingJson: JSON.stringify(smallRing),
  smallFirstElapsed: smallRing[0].elapsedSec,
  smallLastElapsed: smallRing[smallRing.length - 1].elapsedSec,
  bigMax: DIAG_MAX_ROWS,
  bigCount: 250,
  bigLen: bigRing.length,
  bigFirstElapsed: bigRing[0].elapsedSec,
  bigLastElapsed: bigRing[bigRing.length - 1].elapsedSec,
};

// =====================================================================
// Fixture 4 — prior-trail surfacing on load (index.html:6489-6497). Given the
// stored trail, compute the peak heap and the last-5 per-row summary lines.
// `peakHeap` = `Math.max(...trail.map(r => r.heapUsedMB || 0))` (null->0).
// `summaryLine(r)` = the per-row `t+Ns heap=...MB ctxLen=... recycle=... items=...
// words=... stepMs=...[ DEVICE-LOST:...]` string. `headline` = the
// `[DIAG] prior trail: N rows, last t+Es, peak heap PMB.` sentence (peak is
// `.toFixed(0)`).
// =====================================================================
function summaryLine(r) {
  return (
    `t+${r.elapsedSec}s heap=${r.heapUsedMB}MB ctxLen=${r.ctxLen} ` +
    `recycle=${r.recycle} items=${r.items} words=${r.words} ` +
    `stepMs=${r.lastStepMs}` +
    (r.deviceLost ? " DEVICE-LOST:" + r.deviceLost : "")
  );
}
// Build a trail from the tick inputs (reuse the 5 rows above).
const trail = tickInputs.map(buildRow);
const peakHeap = Math.max(...trail.map((r) => r.heapUsedMB || 0));
const last5 = trail.slice(-5);
const trailFixture = {
  rowCount: trail.length,
  lastElapsed: trail[trail.length - 1].elapsedSec,
  peakHeap, // raw number (heapUsedMB units)
  peakHeapHeadline: peakHeap.toFixed(0), // the `.toFixed(0)` used in the headline
  headline:
    `[DIAG] prior trail: ${trail.length} rows, ` +
    `last t+${trail[trail.length - 1].elapsedSec}s, ` +
    `peak heap ${peakHeap.toFixed(0)}MB.`,
  summaryLines: last5.map(summaryLine),
  // also pin a per-row summary for EVERY row (so device-lost + null-heap rows
  // are covered, not just the last-5 window).
  allSummaryLines: trail.map(summaryLine),
};

// =====================================================================
// Write it all out.
// =====================================================================
const fixture = {
  description:
    "DOM/clock-free goldens for the index.html crash-diagnostics ring buffer " +
    "(notetakerDiag): row shape + key order, toFixed(1)/JSON.stringify " +
    "normalization, bounded-ring eviction, and prior-trail surfacing.",
  diagKey: DIAG_KEY,
  diagMaxRows: DIAG_MAX_ROWS,
  diagIntervalMs: DIAG_INTERVAL_MS,
  oneDecimal: oneDecimalCases,
  heapBytes: heapByteCases,
  rows: rowFixtures,
  ring: ringFixtures,
  trail: trailFixture,
};

mkdirSync(join(HERE, "..", "diag"), { recursive: true });
const p = join(HERE, "..", "diag", "diag.json");
writeFileSync(p, JSON.stringify(fixture, null, 1) + "\n");
console.log(
  "wrote",
  p,
  "—",
  oneDecimalCases.length,
  "oneDecimal +",
  heapByteCases.length,
  "heapBytes +",
  rowFixtures.length,
  "rows +",
  "ring(small/big) +",
  trailFixture.allSummaryLines.length,
  "trail lines"
);
