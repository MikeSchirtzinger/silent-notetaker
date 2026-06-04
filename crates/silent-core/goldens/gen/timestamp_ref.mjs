// Reference timestamp formatters — a faithful, DOM-free port of the JS in
// index.html. This is the BEHAVIOR CONTRACT the Rust port (silent-core
// `timestamp` module) must reproduce exactly.
//
// Run: node timestamp_ref.mjs  → writes ../timestamp/*.json
//
// JS sources captured (index.html on hn-prep, current HEAD):
//
//   formatMs(ms) {                                            // "elapsed" core
//     const total = Math.floor(ms / 1000);
//     const m = Math.floor(total / 60);
//     const s = total % 60;
//     return `${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
//   }
//
//   formatStamp(tsMs) {                                       // per-line stamp
//     const fmt = loadSettings().timeFormat || 'elapsed';
//     if (fmt === 'clock') {
//       return new Date(tsMs).toLocaleTimeString('en-US', { hour:'numeric', minute:'2-digit' });
//     }
//     if (fmt === 'ago') {
//       const sec = Math.max(0, Math.floor((Date.now() - tsMs) / 1000));
//       if (sec < 60) return `${sec}s ago`;
//       const m = Math.floor(sec / 60);
//       if (m < 60) return `${m}m ago`;
//       return `${Math.floor(m / 60)}h ago`;
//     }
//     return this.formatMs(tsMs - (this.startTime || tsMs));  // elapsed (default)
//   }
//
//   function formatDuration(ms) {                             // history list
//     const total = Math.floor(ms / 1000);
//     const m = Math.floor(total / 60);
//     const s = total % 60;
//     return `${m}m ${s}s`;
//   }
//
// `clock` mode goes through Intl (`toLocaleTimeString`), which is locale- and
// timezone-dependent. The Rust core is DOM-/Intl-free, so the Rust port formats
// from already-broken-down LOCAL clock components (hour-of-day 0..23, minute),
// which is exactly what `new Date(tsMs).getHours()/getMinutes()` yield. To make
// the golden timezone-independent we feed the formatter explicit (hour, minute)
// pairs and record the en-US 12-hour "h:mm AM/PM" string that Intl produces for
// them — verified here against the real Intl implementation.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "..", "timestamp");
mkdirSync(OUT, { recursive: true });

// ── exact JS functions, copied verbatim (sans `this`/DOM) ────────────────────
function formatMs(ms) {
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function formatElapsed(tsMs, startTime) {
  // startTime may be null/0 → `(this.startTime || tsMs)` falls back to tsMs,
  // which makes the elapsed delta 0 → "00:00".
  return formatMs(tsMs - (startTime || tsMs));
}

function formatAgo(tsMs, nowMs) {
  const sec = Math.max(0, Math.floor((nowMs - tsMs) / 1000));
  if (sec < 60) return `${sec}s ago`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ago`;
  return `${Math.floor(m / 60)}h ago`;
}

// `clock` via Intl on a Date built from explicit LOCAL components. We construct
// the Date in local time (new Date(y,mo,d,h,mi)) so getHours()===h, then format
// it the same way index.html does. The (hour, minute) pair is what the Rust port
// receives; the produced string is the contract.
function formatClockFromComponents(hour, minute) {
  const d = new Date(2024, 0, 1, hour, minute, 0, 0);
  return d.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
}

function write(name, obj) {
  writeFileSync(join(OUT, name), JSON.stringify(obj, null, 1) + "\n");
}

// ── elapsed cases ────────────────────────────────────────────────────────────
{
  // [tsMs, startTime]
  const inputs = [
    [0, 0],
    [1000, 0], // start unknown → falls back to tsMs → 00:00
    [5_000, 5_000], // same instant → 00:00
    [10_000, 5_000], // 5s
    [65_000, 5_000], // 1m00s
    [125_500, 5_000], // 2m00s (sub-second floored)
    [5_000, 65_000], // negative delta (ts before start): -60000ms → "-1:00"
    [4_000, 65_000], // -61000ms → JS quirk "-2:-1" (floor + signed %)
    [64_000, 65_000], // -1000ms → JS quirk "-1:-1"
    [64_500, 65_000], // -500ms → JS quirk "-1:-1" (sub-second floored)
    [3_600_000 + 5_000, 5_000], // 60m00s
    [3_661_000, 0], // start unknown → 00:00 (|| fallback)
    [3_661_000, 1_000], // 61m00s
    [905_999, 5_000], // 15m00s (999ms floored off)
  ];
  const cases = inputs.map(([tsMs, startTime]) => ({
    tsMs,
    startTime,
    expected: formatElapsed(tsMs, startTime),
  }));
  write("elapsed.json", {
    description:
      "elapsed mode: formatMs(tsMs - (startTime || tsMs)); startTime 0/null falls back to tsMs",
    mode: "elapsed",
    cases,
  });
}

// ── ago cases ────────────────────────────────────────────────────────────────
{
  // [tsMs, nowMs]
  const NOW = 1_000_000_000_000;
  const inputs = [
    [NOW, NOW], // 0s ago
    [NOW - 1, NOW], // 0s ago (sub-second floored)
    [NOW - 999, NOW], // 0s ago
    [NOW - 1_000, NOW], // 1s ago
    [NOW - 59_000, NOW], // 59s ago
    [NOW - 60_000, NOW], // 1m ago
    [NOW - 119_000, NOW], // 1m ago
    [NOW - 3_599_000, NOW], // 59m ago
    [NOW - 3_600_000, NOW], // 1h ago
    [NOW - 7_200_000, NOW], // 2h ago
    [NOW - 90_000_000, NOW], // 25h ago
    [NOW + 5_000, NOW], // future ts → clamped to 0s ago
  ];
  const cases = inputs.map(([tsMs, nowMs]) => ({
    tsMs,
    nowMs,
    expected: formatAgo(tsMs, nowMs),
  }));
  write("ago.json", {
    description:
      "ago mode: sec=max(0,floor((now-ts)/1000)); <60s→Ns ago; <60m→Nm ago; else Nh ago; future clamps to 0s",
    mode: "ago",
    cases,
  });
}

// ── clock cases ──────────────────────────────────────────────────────────────
{
  // [hour-of-day 0..23, minute]
  const inputs = [
    [0, 0], // 12:00 AM
    [0, 5], // 12:05 AM
    [1, 23], // 1:23 AM
    [9, 9], // 9:09 AM
    [11, 59], // 11:59 AM
    [12, 0], // 12:00 PM
    [12, 30], // 12:30 PM
    [13, 23], // 1:23 PM
    [15, 5], // 3:05 PM
    [23, 59], // 11:59 PM
  ];
  const cases = inputs.map(([hour, minute]) => ({
    hour,
    minute,
    expected: formatClockFromComponents(hour, minute),
  }));
  write("clock.json", {
    description:
      "clock mode: en-US 12-hour 'h:mm AM/PM' from local (hour-of-day, minute); via Intl in JS",
    mode: "clock",
    cases,
  });
}

// ── duration cases (history list `formatDuration`) ───────────────────────────
{
  const inputs = [
    0,
    500, // <1s → 0m 0s
    1_000, // 0m 1s
    59_000, // 0m 59s
    60_000, // 1m 0s
    61_000, // 1m 1s
    125_000, // 2m 5s
    3_600_000, // 60m 0s
    3_661_000, // 61m 1s
    7_325_500, // 122m 5s (sub-second floored)
  ];
  const cases = inputs.map((ms) => ({ ms, expected: formatDuration(ms) }));
  write("duration.json", {
    description:
      "history-list duration: total=floor(ms/1000); `${floor(total/60)}m ${total%60}s`",
    cases,
  });
}

function formatDuration(ms) {
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}m ${s}s`;
}

console.log("wrote timestamp goldens to", OUT);
