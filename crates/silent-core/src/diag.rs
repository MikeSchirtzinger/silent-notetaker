//! Crash-diagnostics ring buffer (PRD Phase 5, Appendix A row 34).
//!
//! A faithful, browser-free port of the `Diag` sampler in `index.html`
//! (~1899-2005) and the prior-trail surfacing on load (~6480-6504). The original
//! goal was ground truth about the ~5-minute Voxtral lock-up: sample on a TIMER
//! only (never per token) into a bounded `localStorage` ring so the trail
//! SURVIVES a freeze + reload, then inspect it after a crash with
//! `window.dumpDiag()`.
//!
//! # What lives here (policy) vs. what is wiring (later)
//!
//! This module is the **policy + format core**:
//!
//! - [`DiagRow`] — the exact `notetakerDiag` row shape and **key order**, so the
//!   serialized bytes are byte-identical to what the JS `JSON.stringify(rows)`
//!   produced. A Rust-written trail and a JS-written trail are interchangeable
//!   (cross-cutover recovery; `dumpDiag()` parses either).
//! - [`OneDecimal`] — the `+(x).toFixed(1)` numeric normalization, reproducing
//!   `Number.prototype.toFixed(1)` (round-half-up on the exact IEEE-754 value)
//!   followed by `JSON.stringify`'s whole-number elision (`12.0` → `12`).
//! - [`Diag`] — the sampler state machine: the loop counters the engine bumps,
//!   the bounded ring (push then evict to [`DIAG_MAX_ROWS`]), the
//!   [`start`](Diag::start) / [`stop`](Diag::stop) semantics, and the
//!   [`sample`](Diag::sample) row build.
//! - [`StorageSink`] — the trait the ring reads/writes through. The real
//!   `localStorage` sink and the `window.dumpDiag` / prior-trail glue are
//!   **wiring** (they live in `silent-web` and the UI); here we provide an
//!   in-memory sink ([`MemSink`]) so the row format is proven byte-identical in
//!   deterministic tests.
//! - [`prior_trail`] — the load-time surfacing contract ([`headline`], per-row
//!   [`summary_line`], [`peak_heap`]) so the engine-status banner the user sees
//!   after a freeze is reproduced exactly.
//!
//! # The host supplies the clock, the DOM, and the GPU signal
//!
//! `silent-core` has no clock, no DOM, and no WebGPU handle (PRD R5 / the
//! browser-free rule). So the wall-clock `iso`, the `elapsed_sec`, the
//! `performance.memory` byte snapshot, the DOM `.transcript-item` / word counts,
//! the Voxtral ring write cursor, and the device-lost message are all **inputs**
//! the host (the wasm subscriber) passes in each tick. The policy here decides
//! what to record, how to normalize it, and how the ring evicts.

use serde::{Deserialize, Serialize};

/// The `localStorage` key the trail is stored under. Preserved EXACTLY so a
/// Rust-written trail and the prior JS-written trail share storage (recovery
/// across the cutover; `window.dumpDiag()` keeps working).
pub const DIAG_KEY: &str = "notetakerDiag";

/// The bounded ring size: at most this many rows are ever kept, oldest evicted
/// first. Matches `DIAG_MAX_ROWS` in `index.html`.
pub const DIAG_MAX_ROWS: usize = 200;

/// The sampler cadence in milliseconds (~3 s). The Rust core does **not** own
/// the timer (the host ticks it); this is exported so the wasm subscriber and
/// docs reference one constant instead of re-inventing it. Matches
/// `DIAG_INTERVAL_MS` in `index.html`.
pub const DIAG_INTERVAL_MS: u32 = 3000;

/// Cap on the recorded device-lost message length, matching the JS
/// `String(msg).slice(0, 120)` in `onDeviceLost`.
pub const DEVICE_LOST_MAX_CHARS: usize = 120;

/// Bytes per mebibyte — the `1048576` divisor the JS `sample()` uses to turn
/// `performance.memory` byte counts into the `heap*MB` fields.
pub const BYTES_PER_MB: f64 = 1_048_576.0;

/// The `tracing` event/field SCHEMA the crash-diagnostics subscriber matches on.
///
/// Crash diagnostics are "formalized on `tracing`" (PRD Phase 5 / R9): the engine
/// code emits the loop signals and the per-tick sample as `tracing` events on a
/// dedicated target, and the wasm subscriber ([`silent-web`]'s `DiagLayer`)
/// translates those into [`Diag`] hook calls + [`Diag::sample`]. The field NAMES
/// are the contract between emitter and subscriber, so they live here (pure
/// string consts — `silent-core` takes no `tracing` dependency; only the names
/// are shared). The subscriber is the only place that needs `tracing-subscriber`.
///
/// Event kinds (all on [`TARGET`](schema::TARGET), discriminated by the
/// [`KIND`](schema::KIND) field):
///
/// | `diag.kind` | fields | drives |
/// |---|---|---|
/// | `loop_iter` | `input_tokens: u64` | [`Diag::on_loop_iter`] |
/// | `recycle` | — | [`Diag::on_recycle`] |
/// | `put` | `now_ms: f64`, `n_tokens: u64` | [`Diag::on_put`] |
/// | `device_lost` | `message: &str` | [`Diag::on_device_lost`] + an out-of-band [`Diag::sample`] |
/// | `sample` | the [`SampleInput`] fields | [`Diag::sample`] (the ~3 s tick) |
/// | `stats` | the [`crate::EngineStats`] fields | PerfMonitor (row 35), see [`schema::STATS_FIELDS`] |
///
/// The `sample` event carries the host-only values the core cannot produce
/// (`iso`, `elapsed_sec`, the `heap_*` bytes, `items`, `words`, `write_abs`); the
/// loop counters come from the prior `loop_iter`/`recycle`/`put` events the
/// subscriber already folded into its [`Diag`]. The `stats` event is the
/// PerfMonitor telemetry (row 35) traveling on the same target as a `tracing`
/// span/event so one subscriber sees both the crash trail and the perf snapshot.
pub mod schema {
    /// The `tracing` target every diag event is emitted on. The subscriber
    /// filters to this target so unrelated spans never touch the ring.
    pub const TARGET: &str = "silent.diag";

    /// The discriminant field naming the event kind (`loop_iter`, `recycle`,
    /// `put`, `device_lost`, `sample`, `stats`).
    ///
    /// A plain identifier (not a dotted `diag.kind`): the `tracing` event macro
    /// only accepts a dynamic value for a field whose name is a valid Rust
    /// identifier, and the subscriber matches on the recorded field name.
    pub const KIND: &str = "diag_kind";

    /// `diag.kind` value for an [`super::Diag::on_loop_iter`] signal.
    pub const KIND_LOOP_ITER: &str = "loop_iter";
    /// `diag.kind` value for an [`super::Diag::on_recycle`] signal.
    pub const KIND_RECYCLE: &str = "recycle";
    /// `diag.kind` value for an [`super::Diag::on_put`] signal.
    pub const KIND_PUT: &str = "put";
    /// `diag.kind` value for an [`super::Diag::on_device_lost`] signal.
    pub const KIND_DEVICE_LOST: &str = "device_lost";
    /// `diag.kind` value for a sampler tick ([`super::Diag::sample`]).
    pub const KIND_SAMPLE: &str = "sample";
    /// `diag.kind` value for an [`crate::EngineStats`] PerfMonitor snapshot.
    pub const KIND_STATS: &str = "stats";

    /// Field: prompt token count on a `loop_iter` event.
    pub const F_INPUT_TOKENS: &str = "input_tokens";
    /// Field: monotonic `performance.now()` ms on a `put` event.
    pub const F_NOW_MS: &str = "now_ms";
    /// Field: token count on a `put` event.
    pub const F_N_TOKENS: &str = "n_tokens";
    /// Field: device-lost message on a `device_lost` event.
    pub const F_MESSAGE: &str = "message";

    // ---- `sample` event fields (the host-supplied SampleInput) ----
    /// Field: wall-clock ISO timestamp on a `sample` event.
    pub const F_ISO: &str = "iso";
    /// Field: whole seconds since trail start on a `sample` event.
    pub const F_ELAPSED_SEC: &str = "elapsed_sec";
    /// Field: `usedJSHeapSize` bytes (omit/NaN when memory is unavailable).
    pub const F_HEAP_USED: &str = "heap_used_bytes";
    /// Field: `totalJSHeapSize` bytes.
    pub const F_HEAP_TOTAL: &str = "heap_total_bytes";
    /// Field: `jsHeapSizeLimit` bytes.
    pub const F_HEAP_LIMIT: &str = "heap_limit_bytes";
    /// Field: whether `performance.memory` was present this tick (drives the
    /// null-heap rows; carried explicitly because a `tracing` field cannot be
    /// `None`).
    pub const F_HEAP_PRESENT: &str = "heap_present";
    /// Field: DOM `.transcript-item` count on a `sample` event.
    pub const F_ITEMS: &str = "items";
    /// Field: DOM `.transcript-word` count on a `sample` event.
    pub const F_WORDS: &str = "words";
    /// Field: Voxtral ring write cursor on a `sample` event (`-1` ≈ "no ring",
    /// the subscriber maps it to `None`/`null`).
    pub const F_WRITE_ABS: &str = "write_abs";

    /// The [`crate::EngineStats`] field names carried on a `stats` event (row
    /// 35). Listed so the subscriber and the PerfMonitor read one contract.
    pub const STATS_FIELDS: &[&str] = &[
        "load_ms",
        "chunks",
        "avg_chunk_ms",
        "last_chunk_ms",
        "audio_secs",
        "rtf",
        "ttft_ms",
        "pending_samples",
    ];
}

// ===========================================================================
// OneDecimal — the `+(x).toFixed(1)` normalization, serialized as JS would.
// ===========================================================================

/// A number rounded to one decimal place the way JavaScript's
/// `+(x).toFixed(1)` is, then serialized the way `JSON.stringify` serializes the
/// result.
///
/// Two behaviors must be reproduced for byte-identity with the JS trail:
///
/// 1. **`Number.prototype.toFixed(1)`** rounds to one decimal using
///    *round-half-up on the exact IEEE-754 value* of the input. This differs
///    from Rust's `{:.1}` formatter (round-half-to-**even**) only on exact
///    binary-representable ties such as `0.25`: JS yields `0.3`, Rust's
///    formatter yields `0.2`. [`from_f64`](OneDecimal::from_f64) implements the
///    JS rule.
/// 2. **`JSON.stringify`** drops a trailing `.0` (`12.0` serializes as `12`,
///    `45.5` as `45.5`). The [`Serialize`] impl emits a JSON **number** with
///    that elision, and [`Display`](std::fmt::Display) mirrors the JS string
///    interpolation (`${x}`) used in the prior-trail summary lines.
///
/// Internally the value is the signed count of tenths (`value * 10`), an exact
/// integer once rounded, so serialization is exact and lossless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct OneDecimal {
    /// The rounded value expressed as tenths (`round(x * 10)`). Exact.
    tenths: i64,
}

impl OneDecimal {
    /// Round an `f64` to one decimal place using JavaScript's `toFixed(1)` rule
    /// (round-half-up on the exact IEEE-754 value, ties toward +∞).
    ///
    /// Non-finite inputs (`NaN`, `±∞`) cannot occur on the diag path (the
    /// values are byte counts and millisecond deltas) but are mapped to `0` so
    /// the type is total and never panics.
    #[must_use]
    pub fn from_f64(x: f64) -> Self {
        if !x.is_finite() {
            return Self { tenths: 0 };
        }
        Self {
            tenths: to_fixed_1_tenths(x),
        }
    }

    /// Construct from an already-known tenths count (used by tests and by the
    /// host when it pre-rounded; rarely needed directly).
    #[must_use]
    pub const fn from_tenths(tenths: i64) -> Self {
        Self { tenths }
    }

    /// The rounded value as an `f64` (`tenths / 10`). Lossless for the
    /// magnitudes the diag path produces.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "tenths is a small integer (heap MB and ms deltas, far below \
                  2^52); i64 -> f64 is exact here"
    )]
    pub fn as_f64(self) -> f64 {
        self.tenths as f64 / 10.0
    }

    /// Whether the rounded value is a whole number (tenths digit is zero), i.e.
    /// whether `JSON.stringify` would elide the decimal.
    #[must_use]
    pub const fn is_whole(self) -> bool {
        self.tenths % 10 == 0
    }
}

/// ECMAScript `Number.prototype.toFixed(1)` as the integer count of tenths.
///
/// The spec picks the integer `n` minimizing `|n/10 − x|` over the *exact* real
/// value of the `f64` `x`, breaking ties toward the larger `n` (round-half-up).
///
/// The decision hinges on whether the fractional part beyond the tenths digit is
/// below, exactly at, or above one half-tenth (`0.05`). Crucially this must be
/// judged on the EXACT value of the `f64`, not on a 2-decimal rounding of it:
/// `0.15` is stored as `0.1499999…`, so its part-beyond-tenths is just under
/// `0.05` and JS rounds it DOWN to `0.1` — but rounding `0.15` to two decimals
/// first gives `0.15`, which would wrongly round UP. So we expand the value to
/// enough fractional digits (`{:.30}`) that the exact f64 is fully captured
/// (an f64's value has a finite decimal expansion well within 30 places for our
/// magnitudes), then read the tenths digit and inspect everything after it.
fn to_fixed_1_tenths(x: f64) -> i64 {
    let neg = x.is_sign_negative();
    let mag = x.abs();
    // Exact decimal expansion of the f64 to 30 fractional digits. Rust's
    // formatter is correctly rounded, and 30 places far exceeds the digits an
    // f64 of these magnitudes needs, so this string is the exact value (no
    // residual rounding error to mislead the half decision).
    let s = format!("{mag:.30}");
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s.as_str(), ""));
    let int_val: i64 = int_part.parse().unwrap_or(0);
    let fb = frac_part.as_bytes();
    let tenth_digit = i64::from(fb.first().copied().unwrap_or(b'0') - b'0');
    // The hundredths digit of the EXACT expansion decides the half. Because the
    // expansion is exact, `0.15` reads as `…14999…` (hundredths = 4 -> down) and
    // `0.25` reads as `…25000…` (hundredths = 5 -> up, the round-half-up tie).
    // Anything at/above 5 here means the part beyond the tenths is >= 0.05, so
    // round-half-up rounds up; below 5 rounds down.
    let hundredth_digit = i64::from(fb.get(1).copied().unwrap_or(b'0') - b'0');

    let mut tenths = int_val * 10 + tenth_digit;
    if hundredth_digit >= 5 {
        tenths += 1;
    }
    if neg { -tenths } else { tenths }
}

impl std::fmt::Display for OneDecimal {
    /// Mirrors JS `${x}` string interpolation of `+(x).toFixed(1)`: a whole
    /// number prints without a decimal (`38`), otherwise with exactly one
    /// fractional digit (`45.5`). Negative values keep their sign.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_whole() {
            write!(f, "{}", self.tenths / 10)
        } else {
            let neg = self.tenths < 0;
            let mag = self.tenths.unsigned_abs();
            if neg {
                f.write_str("-")?;
            }
            write!(f, "{}.{}", mag / 10, mag % 10)
        }
    }
}

impl Serialize for OneDecimal {
    /// Serialize as a JSON **number**, matching `JSON.stringify(+x.toFixed(1))`:
    /// an integer when the tenths digit is zero (`12`), otherwise a one-decimal
    /// float (`45.5`).
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.is_whole() {
            s.serialize_i64(self.tenths / 10)
        } else {
            // serde_json renders this f64 as `45.5` (shortest round-trip form);
            // for a one-decimal value that is exactly the JS form.
            s.serialize_f64(self.as_f64())
        }
    }
}

impl<'de> Deserialize<'de> for OneDecimal {
    /// Parse from a JSON number. Used when reading a trail back (the value was
    /// already `toFixed(1)`-normalized when written, so re-normalizing is a
    /// no-op but keeps the round-trip total).
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let x = f64::deserialize(d)?;
        Ok(Self::from_f64(x))
    }
}

// ===========================================================================
// DiagRow — the exact notetakerDiag row (shape + KEY ORDER are load-bearing).
// ===========================================================================

/// One sampled row of the `notetakerDiag` ring.
///
/// The **field order here is load-bearing**: serde serializes struct fields in
/// declaration order, and that order — plus the camelCase `rename_all` and the
/// `null`-eliding `Option`s — is what makes `serde_json::to_string(&rows)`
/// byte-identical to the JS `JSON.stringify(rows)`. The order matches the JS
/// `sample()` object literal verbatim:
/// `iso, elapsedSec, heapUsedMB, heapTotalMB, heapLimitMB, items, words,
/// loopIter, recycle, ctxLen, genStepsTotal, inputTokens, lastStepMs,
/// deviceLost, writeAbs`.
///
/// Heap fields and `writeAbs` are `Option`: JS writes `null` when
/// `performance.memory` is unavailable (Firefox/Safari) or when there is no
/// Voxtral ring yet. serde renders `None` as `null` in the same slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DiagRow {
    /// Wall-clock ISO-8601 timestamp of the sample (host-supplied; the core has
    /// no clock). Matches `new Date(now).toISOString()`.
    pub iso: String,
    /// Whole seconds since the trail started (host-supplied). Matches
    /// `Math.round((now - startMs) / 1000)`.
    pub elapsed_sec: u64,
    /// `performance.memory.usedJSHeapSize` in MB, `null` when unavailable.
    ///
    /// Explicitly renamed: the JS key is `heapUsedMB` (capital `B`), whereas
    /// serde's `camelCase` of `heap_used_mb` would emit `heapUsedMb`. The
    /// byte-identity contract requires the exact JS key, so it is pinned here
    /// (and the ts-rs `rename` keeps the generated TypeScript field in sync).
    #[serde(rename = "heapUsedMB")]
    #[cfg_attr(test, ts(rename = "heapUsedMB", type = "number | null"))]
    pub heap_used_mb: Option<OneDecimal>,
    /// `performance.memory.totalJSHeapSize` in MB, `null` when unavailable.
    #[serde(rename = "heapTotalMB")]
    #[cfg_attr(test, ts(rename = "heapTotalMB", type = "number | null"))]
    pub heap_total_mb: Option<OneDecimal>,
    /// `performance.memory.jsHeapSizeLimit` in MB, `null` when unavailable.
    #[serde(rename = "heapLimitMB")]
    #[cfg_attr(test, ts(rename = "heapLimitMB", type = "number | null"))]
    pub heap_limit_mb: Option<OneDecimal>,
    /// DOM `.transcript-item` count (host-supplied; the core has no DOM).
    pub items: u64,
    /// DOM `.transcript-word` count (host-supplied).
    pub words: u64,
    /// Fresh `generate()` contexts started this session (outer-while count).
    pub loop_iter: u64,
    /// `generate()` returns that hit the recycle cap (sawtooth events).
    pub recycle: u64,
    /// Token count emitted in the CURRENT generate context.
    pub ctx_len: u64,
    /// Total streamer `put()` calls across the whole session.
    pub gen_steps_total: u64,
    /// `input_ids` length of the current context (prompt size).
    pub input_tokens: u64,
    /// Wall-ms of the most recent `put()`-to-`put()` gap (`+(dt).toFixed(1)`).
    #[cfg_attr(test, ts(type = "number"))]
    pub last_step_ms: OneDecimal,
    /// Device-lost / OOM message if observed, else empty string.
    pub device_lost: String,
    /// The Voxtral ring write cursor (host-supplied), `null` when there is no
    /// ring yet. Matches `app.tm.voxtralRing.writeAbs`.
    pub write_abs: Option<u64>,
}

// ===========================================================================
// Sample input — the per-tick host-supplied values the core normalizes.
// ===========================================================================

/// The `performance.memory` byte snapshot the host passes in. Absent
/// (`None`) on engines/browsers that do not expose it (Firefox/Safari), which
/// the JS modeled as `performance.memory ? … : null`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeapBytes {
    /// `usedJSHeapSize` in bytes.
    pub used: f64,
    /// `totalJSHeapSize` in bytes.
    pub total: f64,
    /// `jsHeapSizeLimit` in bytes.
    pub limit: f64,
}

/// The host-supplied inputs for one [`Diag::sample`] tick: the values the Rust
/// core cannot produce itself (clock, DOM, GPU heap, Voxtral ring cursor). The
/// loop counters ([`Diag`] internal) are read by the sampler; they are NOT part
/// of this struct.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleInput {
    /// Wall-clock ISO timestamp (`new Date(now).toISOString()`).
    pub iso: String,
    /// Whole seconds since the trail started.
    pub elapsed_sec: u64,
    /// `performance.memory` byte snapshot, or `None` when unavailable.
    pub heap: Option<HeapBytes>,
    /// DOM `.transcript-item` count.
    pub items: u64,
    /// DOM `.transcript-word` count.
    pub words: u64,
    /// Voxtral ring write cursor, or `None`.
    pub write_abs: Option<u64>,
}

// ===========================================================================
// StorageSink — the trait the ring reads/writes through (localStorage = wiring).
// ===========================================================================

/// The persistence boundary for the diag ring.
///
/// The real implementation is `localStorage` (wiring in `silent-web` / the UI);
/// the policy here only needs to read the prior trail and replace it with the
/// new one. The trait carries **typed [`DiagRow`]s**, not a JSON string: the
/// JSON↔storage encoding (`JSON.stringify(rows)` / `JSON.parse`) is the
/// implementor's job, exactly as `silent-web` already owns `serde_json` for
/// every other boundary type while `silent-core` stays dependency-free (PRD:
/// "silent-core must not depend on browser APIs" — and not on a JSON codec it
/// does not need for its logic).
///
/// The byte-identity of that JSON with the JS `notetakerDiag` value is a
/// property of [`DiagRow`]'s field order + [`OneDecimal`] serialization, and is
/// proven against the JS golden in `tests/diag_golden.rs` (which DOES serialize,
/// via the dev-only `serde_json`). So the contract — "a Rust-written trail and a
/// JS-written trail are interchangeable" — is enforced at the type, and the sink
/// is just the seam.
pub trait StorageSink {
    /// Read the prior trail for `key`, or an empty vec if absent / unreadable
    /// (the implementor maps a `JSON.parse` failure to empty, matching the JS
    /// `try { JSON.parse(…) } catch { return [] }`).
    fn read(&self, key: &str) -> Vec<DiagRow>;

    /// Replace the stored trail for `key`. A failure (quota) is swallowed by the
    /// implementor, matching the JS `try { setItem } catch { /* drop */ }`; the
    /// trait returns `()` so the policy never branches on storage failure.
    fn write(&mut self, key: &str, rows: &[DiagRow]);
}

/// An in-memory [`StorageSink`] for deterministic tests and the native build: it
/// holds the last-written rows per key, so a test can drive the [`Diag`] sampler
/// and then read back the exact rows. The golden test serializes these rows with
/// `serde_json` and compares the bytes to the JS `JSON.stringify(rows)` fixture,
/// proving the row format is byte-identical without a browser.
#[derive(Debug, Default, Clone)]
pub struct MemSink {
    store: std::collections::BTreeMap<String, Vec<DiagRow>>,
}

impl MemSink {
    /// A fresh empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current rows for `key` (what a `dumpDiag()` would parse back).
    #[must_use]
    pub fn get(&self, key: &str) -> &[DiagRow] {
        self.store.get(key).map_or(&[], Vec::as_slice)
    }
}

impl StorageSink for MemSink {
    fn read(&self, key: &str) -> Vec<DiagRow> {
        self.store.get(key).cloned().unwrap_or_default()
    }

    fn write(&mut self, key: &str, rows: &[DiagRow]) {
        self.store.insert(key.to_owned(), rows.to_vec());
    }
}

// ===========================================================================
// Diag — the sampler state machine.
// ===========================================================================

/// The loop counters the engine increments; the sampler only READS them. Mirror
/// of the JS closure object `c` in `index.html`.
#[derive(Debug, Clone, PartialEq, Default)]
struct Counters {
    loop_iter: u64,
    recycle: u64,
    gen_steps_total: u64,
    ctx_len: u64,
    /// Internal: tracks the previous `put()` timestamp to derive `last_step_ms`.
    /// `None` until the first `put()`.
    last_put_at: Option<f64>,
    last_step_ms: OneDecimal,
    input_tokens: u64,
    device_lost: String,
}

/// The crash-diagnostics sampler.
///
/// Holds the loop counters the engine bumps via the cheap hooks
/// ([`on_loop_iter`](Diag::on_loop_iter), [`on_recycle`](Diag::on_recycle),
/// [`on_put`](Diag::on_put), [`on_device_lost`](Diag::on_device_lost)) and the
/// bounded ring it writes through a [`StorageSink`]. The host owns the ~3 s
/// timer and the wall clock: it calls [`sample`](Diag::sample) on each tick with
/// a [`SampleInput`], and [`start`](Diag::start) / [`stop`](Diag::stop) at
/// session edges.
///
/// This is the unchanged behavior of the JS `Diag` closure, with the host-only
/// concerns (timer, clock, DOM, GPU heap) lifted out as inputs.
#[derive(Debug, Clone, Default)]
pub struct Diag {
    counters: Counters,
}

impl Diag {
    /// A fresh sampler with zeroed counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a fresh trail (called when a Voxtral session starts): zero the
    /// counters and CLEAR the prior trail in storage so the next crash trail is
    /// clean. Mirrors the JS `start()` reset + `write(DIAG_KEY, [])`. The host
    /// then schedules the timer and takes the baseline [`sample`](Diag::sample)
    /// at t=0 (the JS `start()` calls `sample()` once itself; here the host does
    /// it explicitly so the clock value is supplied).
    pub fn start<S: StorageSink>(&mut self, sink: &mut S) {
        self.counters = Counters::default();
        // Clear the prior trail: replace it with an empty ring. The sink encodes
        // this as the JS `localStorage.setItem(KEY, '[]')` bytes.
        sink.write(DIAG_KEY, &[]);
    }

    /// Stop sampling (host clears the timer). The JS `stop()` takes one last
    /// sample; here the host supplies the final [`SampleInput`] and calls
    /// [`sample`](Diag::sample) itself if it wants that final row, so this only
    /// needs to exist for symmetry / future state. Kept as a no-op marker so the
    /// call site reads the same as the JS.
    pub fn stop(&self) {
        // Intentionally empty: the final sample is taken by the host via
        // `sample()` with a supplied clock, exactly as `start()`'s baseline is.
    }

    /// Loop-side hook: a NEW `generate()` context is starting. Bumps
    /// `loop_iter`, resets `ctx_len`, and records the prompt token count.
    /// Cheap, no allocation. Mirrors JS `onLoopIter(inputTokens)`.
    pub fn on_loop_iter(&mut self, input_tokens: u64) {
        self.counters.loop_iter += 1;
        self.counters.ctx_len = 0;
        self.counters.input_tokens = input_tokens;
    }

    /// Loop-side hook: a `generate()` return hit the recycle cap. Bumps
    /// `recycle`. Mirrors JS `onRecycle()`.
    pub fn on_recycle(&mut self) {
        self.counters.recycle += 1;
    }

    /// Loop-side hook: one streamer `put()` happened. Updates `last_step_ms`
    /// from the supplied monotonic clock (`performance.now()` — host-supplied,
    /// the core has no clock), advances `ctx_len` by `n_tokens` (min 1, matching
    /// the JS `nTokens || 1`), and bumps `gen_steps_total`. Mirrors JS
    /// `onPut(nTokens)`.
    pub fn on_put(&mut self, now: f64, n_tokens: u64) {
        if let Some(prev) = self.counters.last_put_at {
            self.counters.last_step_ms = OneDecimal::from_f64(now - prev);
        }
        self.counters.last_put_at = Some(now);
        self.counters.ctx_len += n_tokens.max(1);
        self.counters.gen_steps_total += 1;
    }

    /// Loop-side hook: a WebGPU device-lost / OOM error was observed (which
    /// `performance.memory` CANNOT see). Records the message (capped to
    /// [`DEVICE_LOST_MAX_CHARS`] chars, matching the JS `.slice(0, 120)`). The
    /// JS also took an immediate out-of-band sample; here the host takes that
    /// sample after calling this (it supplies the clock), so this hook only
    /// stores the message. Mirrors JS `onDeviceLost(msg)` minus the
    /// host-clocked sample.
    pub fn on_device_lost(&mut self, msg: &str) {
        let m = if msg.is_empty() { "device-lost" } else { msg };
        self.counters.device_lost = m.chars().take(DEVICE_LOST_MAX_CHARS).collect();
    }

    /// Build the [`DiagRow`] for the current counter state and the supplied
    /// per-tick [`SampleInput`], push it onto the bounded ring read from `sink`,
    /// evict to [`DIAG_MAX_ROWS`], and write the ring back. Returns the row that
    /// was recorded (the JS `sample()` returns the row too). Mirrors JS
    /// `sample()` end-to-end.
    ///
    /// The read-modify-write is the exact JS sequence: read the prior rows (an
    /// unreadable value is treated as empty), `push`, `while len > MAX shift`,
    /// write. The serialized bytes are byte-identical to the JS
    /// `JSON.stringify(rows)` (proven by the golden test).
    pub fn sample<S: StorageSink>(&self, input: &SampleInput, sink: &mut S) -> DiagRow {
        let row = self.build_row(input);
        let mut rows = sink.read(DIAG_KEY);
        push_bounded(&mut rows, row.clone());
        sink.write(DIAG_KEY, &rows);
        row
    }

    /// Build the row for the current counters + supplied input WITHOUT touching
    /// storage. Exposed so the wasm subscriber (or a test) can obtain the typed
    /// row independently of the ring write. The heap byte snapshot is normalized
    /// to `*MB` here (the `/1048576` + `toFixed(1)` the JS did inline).
    #[must_use]
    pub fn build_row(&self, input: &SampleInput) -> DiagRow {
        let (used, total, limit) = match input.heap {
            Some(h) => (
                Some(OneDecimal::from_f64(h.used / BYTES_PER_MB)),
                Some(OneDecimal::from_f64(h.total / BYTES_PER_MB)),
                Some(OneDecimal::from_f64(h.limit / BYTES_PER_MB)),
            ),
            None => (None, None, None),
        };
        DiagRow {
            iso: input.iso.clone(),
            elapsed_sec: input.elapsed_sec,
            heap_used_mb: used,
            heap_total_mb: total,
            heap_limit_mb: limit,
            items: input.items,
            words: input.words,
            loop_iter: self.counters.loop_iter,
            recycle: self.counters.recycle,
            ctx_len: self.counters.ctx_len,
            gen_steps_total: self.counters.gen_steps_total,
            input_tokens: self.counters.input_tokens,
            last_step_ms: self.counters.last_step_ms,
            device_lost: self.counters.device_lost.clone(),
            write_abs: input.write_abs,
        }
    }
}

/// Push `row` onto the bounded ring and evict from the front until the length is
/// at most [`DIAG_MAX_ROWS`]. This is the exact JS `rows.push(row); while
/// (rows.length > DIAG_MAX_ROWS) rows.shift();` — a `push` followed by
/// front-eviction, so the ring keeps the most recent [`DIAG_MAX_ROWS`] rows in
/// order. Exposed so the wasm subscriber can apply the same eviction when it
/// drives the ring directly.
pub fn push_bounded(rows: &mut Vec<DiagRow>, row: DiagRow) {
    rows.push(row);
    while rows.len() > DIAG_MAX_ROWS {
        rows.remove(0);
    }
}

// ===========================================================================
// prior_trail — the load-time surfacing contract.
// ===========================================================================

/// The prior-trail surfacing contract (`index.html` ~6480-6504): on load, if a
/// trail survived a freeze, summarize it so the user sees the crash evidence in
/// the engine-status banner without opening `DevTools`.
///
/// These are pure functions of the stored rows; the DOM write (setting
/// `engineStatusText.innerHTML`) is wiring in the UI. The strings here are
/// byte-identical to the JS that built the banner.
pub mod prior_trail {
    use super::DiagRow;

    /// Peak `heapUsedMB` across the trail, treating a `null` heap as `0`
    /// (`Math.max(...trail.map(r => r.heapUsedMB || 0))`). Returned as an `f64`
    /// in heap-MB units. An empty trail yields `0.0`.
    #[must_use]
    pub fn peak_heap(rows: &[DiagRow]) -> f64 {
        rows.iter()
            .map(|r| r.heap_used_mb.map_or(0.0, super::OneDecimal::as_f64))
            .fold(0.0_f64, f64::max)
    }

    /// The headline sentence: `[DIAG] prior trail: N rows, last t+Es, peak heap
    /// PMB.` where `P` is the peak heap rounded with `toFixed(0)` (round-half-up,
    /// matching JS). Returns an empty string for an empty trail (the JS guards
    /// `if (trail.length)` before building it).
    #[must_use]
    pub fn headline(rows: &[DiagRow]) -> String {
        if rows.is_empty() {
            return String::new();
        }
        let last_elapsed = rows[rows.len() - 1].elapsed_sec;
        let peak = peak_heap(rows);
        format!(
            "[DIAG] prior trail: {} rows, last t+{}s, peak heap {}MB.",
            rows.len(),
            last_elapsed,
            to_fixed_0(peak),
        )
    }

    /// The per-row summary line shown in the banner's `<small>` block:
    /// `t+Ns heap=…MB ctxLen=… recycle=… items=… words=… stepMs=…` with a
    /// trailing ` DEVICE-LOST:<msg>` when a device-lost was recorded. A `null`
    /// heap renders literally as `heap=nullMB` (the JS `${r.heapUsedMB}`
    /// interpolation of `null`).
    #[must_use]
    pub fn summary_line(row: &DiagRow) -> String {
        let heap = match row.heap_used_mb {
            Some(v) => v.to_string(),
            None => "null".to_owned(),
        };
        let base = format!(
            "t+{}s heap={}MB ctxLen={} recycle={} items={} words={} stepMs={}",
            row.elapsed_sec, heap, row.ctx_len, row.recycle, row.items, row.words, row.last_step_ms,
        );
        if row.device_lost.is_empty() {
            base
        } else {
            format!("{base} DEVICE-LOST:{}", row.device_lost)
        }
    }

    /// The last-`n` summary lines (the JS surfaced `trail.slice(-5)`), oldest
    /// first. `n` is typically 5.
    #[must_use]
    pub fn last_summary_lines(rows: &[DiagRow], n: usize) -> Vec<String> {
        let start = rows.len().saturating_sub(n);
        rows[start..].iter().map(summary_line).collect()
    }

    /// `Number.prototype.toFixed(0)`: round-half-up on the exact value, no
    /// decimal point. Used for the headline peak-heap figure.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "peak heap is a small non-negative MB figure; after \
                  round-half-up it is well within i64 and non-negative"
    )]
    fn to_fixed_0(x: f64) -> i64 {
        if !x.is_finite() {
            return 0;
        }
        // round-half-up (ties toward +inf), matching JS toFixed(0).
        (x + 0.5).floor() as i64
    }
}

// ===========================================================================
// Unit tests (intent). Byte-identity vs the JS trail is proven separately in
// tests/diag_golden.rs against the committed JS fixture.
// ===========================================================================
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism (PRD lint config)"
)]
mod tests {
    use super::prior_trail;
    use super::{
        DEVICE_LOST_MAX_CHARS, DIAG_KEY, DIAG_MAX_ROWS, Diag, DiagRow, HeapBytes, MemSink,
        OneDecimal, SampleInput, StorageSink, push_bounded,
    };

    #[test]
    fn one_decimal_tie_matches_js_round_half_up() {
        // 0.25 is exactly representable, so it is a true tie. JS toFixed(1)
        // rounds half UP -> 0.3; Rust's `{:.1}` formatter rounds half EVEN ->
        // 0.2. The port must match JS.
        assert_eq!(OneDecimal::from_f64(0.25).to_string(), "0.3");
        // 0.15 / 0.35 / 0.85 are NOT exact (stored just below the half), so they
        // round down in both — pinned to lock the IEEE-754 behavior in.
        assert_eq!(OneDecimal::from_f64(0.15).to_string(), "0.1");
        assert_eq!(OneDecimal::from_f64(0.35).to_string(), "0.3");
        assert_eq!(OneDecimal::from_f64(0.85).to_string(), "0.8");
        // 0.05 is stored just ABOVE the half -> rounds up.
        assert_eq!(OneDecimal::from_f64(0.05).to_string(), "0.1");
    }

    #[test]
    fn one_decimal_whole_number_has_no_decimal() {
        assert_eq!(OneDecimal::from_f64(12.0).to_string(), "12");
        assert_eq!(OneDecimal::from_f64(12.04).to_string(), "12"); // rounds down to 12.0
        assert_eq!(OneDecimal::from_f64(0.0).to_string(), "0");
        assert_eq!(OneDecimal::from_f64(150.0).to_string(), "150");
    }

    #[test]
    fn one_decimal_serializes_as_json_number() {
        // whole -> integer token, fractional -> one-decimal token.
        assert_eq!(
            serde_json::to_string(&OneDecimal::from_f64(50.0)).unwrap(),
            "50"
        );
        assert_eq!(
            serde_json::to_string(&OneDecimal::from_f64(45.5)).unwrap(),
            "45.5"
        );
        assert_eq!(
            serde_json::to_string(&OneDecimal::from_f64(0.0)).unwrap(),
            "0"
        );
        // round-trip through the custom Deserialize is stable.
        let v: OneDecimal = serde_json::from_str("45.5").unwrap();
        assert_eq!(v, OneDecimal::from_f64(45.5));
    }

    #[test]
    fn heap_bytes_normalize_like_js() {
        // 52428800 bytes / 1048576 = 50.0 -> "50"; 89128960 / 1048576 = 85.0 ->
        // "85" (matches the JS golden rows).
        let d = Diag::new();
        let input = SampleInput {
            iso: "x".into(),
            elapsed_sec: 0,
            heap: Some(HeapBytes {
                used: 52_428_800.0,
                total: 67_108_864.0,
                limit: 4_294_967_296.0,
            }),
            items: 0,
            words: 0,
            write_abs: None,
        };
        let row = d.build_row(&input);
        assert_eq!(row.heap_used_mb.unwrap().to_string(), "50");
        assert_eq!(row.heap_total_mb.unwrap().to_string(), "64");
        assert_eq!(row.heap_limit_mb.unwrap().to_string(), "4096");
    }

    #[test]
    fn absent_heap_is_null() {
        let d = Diag::new();
        let input = SampleInput {
            iso: "x".into(),
            elapsed_sec: 9,
            heap: None,
            items: 7,
            words: 51,
            write_abs: Some(144_000),
        };
        let row = d.build_row(&input);
        assert!(row.heap_used_mb.is_none());
        assert!(row.heap_total_mb.is_none());
        assert!(row.heap_limit_mb.is_none());
        // serde renders None as JSON null in the heap slots.
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"heapUsedMB\":null"), "{json}");
    }

    #[test]
    fn key_order_is_load_bearing() {
        // The exact JS sample() key order. serde emits declaration order.
        let d = Diag::new();
        let row = d.build_row(&SampleInput {
            iso: "2026-06-04T17:00:00.000Z".into(),
            elapsed_sec: 0,
            heap: Some(HeapBytes {
                used: 0.0,
                total: 0.0,
                limit: 0.0,
            }),
            items: 0,
            words: 0,
            write_abs: None,
        });
        let json = serde_json::to_string(&row).unwrap();
        let expected_order = [
            "iso",
            "elapsedSec",
            "heapUsedMB",
            "heapTotalMB",
            "heapLimitMB",
            "items",
            "words",
            "loopIter",
            "recycle",
            "ctxLen",
            "genStepsTotal",
            "inputTokens",
            "lastStepMs",
            "deviceLost",
            "writeAbs",
        ];
        let mut last = 0usize;
        for key in expected_order {
            let pat = format!("\"{key}\":");
            let pos = json
                .find(&pat)
                .unwrap_or_else(|| panic!("missing {key} in {json}"));
            assert!(pos >= last, "key {key} out of order in {json}");
            last = pos;
        }
    }

    #[test]
    fn hooks_drive_counters_like_js() {
        let mut d = Diag::new();
        // onLoopIter resets ctxLen and records prompt size.
        d.on_loop_iter(8);
        // onPut: first has no prior timestamp -> lastStepMs stays 0; ctxLen += n.
        d.on_put(1000.0, 3);
        // second put: dt = 41.27 -> "41.3" (the JS golden row 2).
        d.on_put(1041.27, 2);
        d.on_recycle();
        let row = d.build_row(&SampleInput {
            iso: "x".into(),
            elapsed_sec: 3,
            heap: None,
            items: 0,
            words: 0,
            write_abs: None,
        });
        assert_eq!(row.loop_iter, 1);
        assert_eq!(row.input_tokens, 8);
        assert_eq!(row.ctx_len, 5); // 3 + 2
        assert_eq!(row.gen_steps_total, 2);
        assert_eq!(row.recycle, 1);
        assert_eq!(row.last_step_ms.to_string(), "41.3");
    }

    #[test]
    fn on_put_min_one_token() {
        // JS `c.ctxLen += (nTokens || 1)` — a 0-token put still advances by 1.
        let mut d = Diag::new();
        d.on_put(0.0, 0);
        let row = d.build_row(&blank_input());
        assert_eq!(row.ctx_len, 1);
        assert_eq!(row.gen_steps_total, 1);
    }

    #[test]
    fn device_lost_capped_and_empty_defaults() {
        let mut d = Diag::new();
        d.on_device_lost("");
        assert_eq!(d.build_row(&blank_input()).device_lost, "device-lost");
        let long = "x".repeat(300);
        d.on_device_lost(&long);
        let stored = d.build_row(&blank_input()).device_lost;
        assert_eq!(stored.chars().count(), DEVICE_LOST_MAX_CHARS);
    }

    #[test]
    fn start_clears_prior_trail() {
        let mut sink = MemSink::new();
        // Seed a prior trail.
        sink.write(DIAG_KEY, &[blank_row()]);
        assert_eq!(sink.get(DIAG_KEY).len(), 1);
        let mut d = Diag::new();
        d.on_recycle(); // dirty the counters
        d.start(&mut sink);
        // Trail cleared and counters reset.
        assert_eq!(sink.get(DIAG_KEY).len(), 0);
        assert_eq!(d.build_row(&blank_input()).recycle, 0);
    }

    #[test]
    fn sample_appends_through_sink() {
        let mut sink = MemSink::new();
        let d = Diag::new();
        d.sample(&blank_input(), &mut sink);
        d.sample(&blank_input(), &mut sink);
        assert_eq!(sink.get(DIAG_KEY).len(), 2);
    }

    #[test]
    fn ring_evicts_front_at_max() {
        let mut rows: Vec<DiagRow> = Vec::new();
        for i in 0..(DIAG_MAX_ROWS + 50) {
            let mut r = blank_row();
            r.elapsed_sec = i as u64;
            push_bounded(&mut rows, r);
        }
        assert_eq!(rows.len(), DIAG_MAX_ROWS);
        // oldest kept is row #50 (the first 50 were evicted), newest is the last.
        assert_eq!(rows[0].elapsed_sec, 50);
        assert_eq!(
            rows[rows.len() - 1].elapsed_sec,
            (DIAG_MAX_ROWS + 49) as u64
        );
    }

    #[test]
    fn prior_trail_summary_line_and_headline() {
        let mut used = blank_row();
        used.elapsed_sec = 12;
        used.heap_used_mb = Some(OneDecimal::from_f64(2008.0));
        used.ctx_len = 110;
        used.recycle = 3;
        used.items = 9;
        used.words = 70;
        used.last_step_ms = OneDecimal::from_f64(512.9);
        used.device_lost = "Device lost: out of memory".into();
        let line = prior_trail::summary_line(&used);
        assert_eq!(
            line,
            "t+12s heap=2008MB ctxLen=110 recycle=3 items=9 words=70 stepMs=512.9 \
             DEVICE-LOST:Device lost: out of memory"
        );
        // null heap renders literally as `heap=nullMB`.
        let mut nullheap = blank_row();
        nullheap.elapsed_sec = 9;
        nullheap.heap_used_mb = None;
        nullheap.ctx_len = 5;
        nullheap.recycle = 2;
        nullheap.items = 7;
        nullheap.words = 51;
        nullheap.last_step_ms = OneDecimal::from_f64(45.5);
        assert_eq!(
            prior_trail::summary_line(&nullheap),
            "t+9s heap=nullMB ctxLen=5 recycle=2 items=7 words=51 stepMs=45.5"
        );
        // headline: peak heap with toFixed(0).
        let rows = vec![nullheap, used];
        assert!((prior_trail::peak_heap(&rows) - 2008.0).abs() < 1e-9);
        assert_eq!(
            prior_trail::headline(&rows),
            "[DIAG] prior trail: 2 rows, last t+12s, peak heap 2008MB."
        );
    }

    #[test]
    fn empty_trail_headline_is_blank() {
        assert_eq!(prior_trail::headline(&[]), "");
        assert!(prior_trail::peak_heap(&[]).abs() < 1e-9);
    }

    fn blank_input() -> SampleInput {
        SampleInput {
            iso: "2026-06-04T17:00:00.000Z".into(),
            elapsed_sec: 0,
            heap: None,
            items: 0,
            words: 0,
            write_abs: None,
        }
    }

    fn blank_row() -> DiagRow {
        Diag::new().build_row(&blank_input())
    }
}
