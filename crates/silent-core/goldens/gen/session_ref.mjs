// Reference session formatters — a faithful, DOM-free port of the JS in
// index.html that the silent-core session machine ports to Rust. This file is
// the BEHAVIOR CONTRACT: the Rust `format_ms` / `format_ago` / `resolve_title`
// / `clamp_title` ports must reproduce these outputs byte-for-byte.
//
// Sourced verbatim from index.html:
//   - `formatMs(ms)`                    ~line 4653
//   - `formatStamp(tsMs)` 'ago' branch  ~line 4666 (Date.now()-tsMs delta path)
//   - `formatStamp(tsMs)` 'elapsed'     ~line 4674 (tsMs - (startTime||tsMs))
//   - title `value.trim() || 'Untitled Meeting'` ~line 3970 + maxlength="120"
//     input cap ~line 1387
//
// The 'clock' branch of formatStamp is NOT reproduced here: it calls
// `new Date(tsMs).toLocaleTimeString('en-US', …)`, whose output is owned by the
// host's locale/timezone (the Rust core takes that string from the host and
// echoes it — there is nothing to port and nothing locale-independent to pin).
//
// Run: node session_ref.mjs  → writes ../format/*.json
//
// These are PURE FUNCTIONS of their inputs — no audio, no clock, no DOM — so the
// fixtures are exactly reproducible. The Rust golden test
// (tests/session_golden.rs) replays each case and asserts equality.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// =====================================================================
// Exact JS ports from index.html (copied verbatim, DOM stripped).
// =====================================================================

// index.html:4653 — formatMs(ms): integer seconds, mm:ss zero-padded, minutes
// can exceed 99 (no hours rollover).
function formatMs(ms) {
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

// index.html:4666 — the 'ago' branch of formatStamp, expressed over an explicit
// elapsed-millis delta (the JS computes `Date.now() - tsMs`; here the caller
// passes that delta directly so the function is clock-free and deterministic).
function formatAgo(deltaMs) {
  const sec = Math.max(0, Math.floor(deltaMs / 1000));
  if (sec < 60) return `${sec}s ago`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ago`;
  return `${Math.floor(m / 60)}h ago`;
}

// index.html:4674 — the 'elapsed' branch of formatStamp: formatMs(tsMs - start).
// `start` stands in for `(this.startTime || tsMs)`; pass start === tsMs to model
// the "no active recording" fallback (yields "00:00").
function formatStampElapsed(tsMs, start) {
  return formatMs(tsMs - start);
}

// index.html:3970 — `value.trim() || 'Untitled Meeting'`, then the input's
// maxlength="120" (index.html:1387) caps the stored string. HTML maxlength
// counts UTF-16 code units; we count code points here and the Rust port counts
// Unicode scalar values — these agree for all BMP text (real titles) and the
// astral-plane divergence is documented + tested in the Rust port. The fixtures
// below stay in the regime where they agree, which is what a faithful golden
// pins.
const DEFAULT_TITLE = "Untitled Meeting";
const MAX_TITLE_CHARS = 120;

function resolveTitle(raw) {
  const trimmed = raw.trim();
  const base = trimmed.length === 0 ? DEFAULT_TITLE : trimmed;
  return [...base].slice(0, MAX_TITLE_CHARS).join("");
}

function clampTitle(raw) {
  // The bare input cap: no trim, just maxlength. Used for SetTitle echoes.
  return [...raw].slice(0, MAX_TITLE_CHARS).join("");
}

// Code-point count (matches Rust `chars().count()`), for asserting the cap.
function charCount(s) {
  return [...s].length;
}

// =====================================================================
// Fixture cases. Each entry records inputs and the EXACT JS output, so the
// Rust port can replay the inputs and assert byte-equality.
// =====================================================================

const formatMsCases = [
  0, // 00:00
  999, // 00:00 — sub-second floors
  1_000, // 00:01
  59_000, // 00:59
  60_000, // 01:00
  61_500, // 01:01 — floors the .5s
  600_000, // 10:00
  5_940_000, // 99:00 — last sub-100 minute
  6_000_000, // 100:00 — minutes exceed 99, NO hours rollover
  6_059_000, // 100:59
  362_439_000, // 6040:39 — large value, still mm:ss
].map((ms) => ({ ms, out: formatMs(ms) }));

const formatAgoCases = [
  0, // 0s ago
  -5_000, // 0s ago — negative delta clamps to 0 (Math.max(0, …))
  1_000, // 1s ago
  30_000, // 30s ago
  59_000, // 59s ago
  60_000, // 1m ago — boundary
  90_000, // 1m ago — floors
  59 * 60_000, // 59m ago
  60 * 60_000, // 1h ago — boundary
  90 * 60_000, // 1h ago — floors
  150 * 60_000, // 2h ago — 2.5h floors
  25 * 60 * 60_000, // 25h ago — hours have no day rollover
].map((deltaMs) => ({ deltaMs, out: formatAgo(deltaMs) }));

const formatStampElapsedCases = [
  { tsMs: 31_000, start: 1_000 }, // 00:30
  { tsMs: 1_000, start: 1_000 }, // 00:00 — ts == start (no-active-recording fallback)
  { tsMs: 130_000, start: 10_000 }, // 02:00
  { tsMs: 500, start: 0 }, // 00:00 — sub-second
].map((c) => ({ ...c, out: formatStampElapsed(c.tsMs, c.start) }));

const resolveTitleCases = [
  "", // -> Untitled Meeting
  "   ", // -> Untitled Meeting (whitespace-only)
  "\t\n ", // -> Untitled Meeting
  "Weekly Sync", // -> Weekly Sync
  "  Weekly Sync  ", // -> Weekly Sync (trimmed)
  " ".repeat(5) + "Standup", // -> Standup
  "y".repeat(130), // -> 120 'y' (trim no-op, then cap)
  "  " + "y".repeat(130) + "  ", // -> 120 'y' (trim then cap)
].map((raw) => {
  const out = resolveTitle(raw);
  return { raw, out, outChars: charCount(out) };
});

const clampTitleCases = [
  "Standup", // unchanged
  "x".repeat(200), // -> 120 'x'
  "z".repeat(150), // -> 120 'z'
  "😀".repeat(130), // -> 120 emoji (code-point cap never splits a scalar)
].map((raw) => {
  const out = clampTitle(raw);
  return { raw, out, outChars: charCount(out) };
});

const fixture = {
  description:
    "DOM-free goldens for index.html formatMs/formatStamp(ago,elapsed)/title-resolve/title-clamp",
  defaultTitle: DEFAULT_TITLE,
  maxTitleChars: MAX_TITLE_CHARS,
  formatMs: formatMsCases,
  formatAgo: formatAgoCases,
  formatStampElapsed: formatStampElapsedCases,
  resolveTitle: resolveTitleCases,
  clampTitle: clampTitleCases,
};

mkdirSync(join(HERE, "..", "format"), { recursive: true });
const p = join(HERE, "..", "format", "session_formatters.json");
writeFileSync(p, JSON.stringify(fixture, null, 1) + "\n");
console.log(
  "wrote",
  p,
  "—",
  formatMsCases.length,
  "formatMs +",
  formatAgoCases.length,
  "formatAgo +",
  formatStampElapsedCases.length,
  "formatStampElapsed +",
  resolveTitleCases.length,
  "resolveTitle +",
  clampTitleCases.length,
  "clampTitle cases"
);
