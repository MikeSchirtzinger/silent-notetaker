//! Timestamp display formatting — the three per-line timestamp modes and the
//! history-list duration string, as pure, DOM-free formatting policy.
//!
//! These are a byte-identical port of the JavaScript in `index.html`
//! (`formatMs`, `formatStamp`, `formatDuration`); Appendix A row 24. The UI
//! cycles the active mode with [`crate::commands::UiCommand::CycleTimestampMode`]
//! and re-renders every stamp through these functions.
//!
//! # Byte-identical to the JS, quirks included
//!
//! The shipping code computes `mm:ss` with `Math.floor` and JavaScript's
//! sign-preserving `%`, then zero-pads with `String#padStart(2, '0')`. For
//! negative inputs (a per-line stamp earlier than the recording start — possible
//! when timestamps are re-timed) this produces strings like `-1:00` and even
//! `-2:-59`. That behavior is reproduced exactly here (see [`format_ms`] and the
//! golden fixtures under `goldens/timestamp/`), because the contract is "the UI
//! does not change," not "the arithmetic is tidied up."
//!
//! # No `Date`/`Intl`
//!
//! `clock` mode in the UI goes through `Intl` (`toLocaleTimeString`), which is
//! locale-/timezone-dependent and unavailable in a DOM-free core. The core
//! formats from already-broken-down LOCAL clock components — the
//! `Date#getHours()/getMinutes()` the orchestrator (in `silent-web`) supplies —
//! reproducing the exact en-US 12-hour `h:mm AM/PM` string. The golden
//! `goldens/timestamp/clock.json` was generated against the real `Intl`
//! implementation, so the port is validated against it.

// `TimestampMode` is the single canonical timestamp-mode enum, defined in
// [`crate::commands`] (referenced by `SessionEvent::TimestampModeChanged` and
// driven by `UiCommand::CycleTimestampMode`). H1 and H3 originally each declared
// an equivalent enum; they are reconciled to one definition here. Its cycle
// helpers (`CYCLE`, `next`, `label`/`as_str`) live alongside the type in
// `commands.rs`.
pub use crate::commands::TimestampMode;

/// `mm:ss` from a millisecond delta, byte-identical to the JS `formatMs`:
///
/// ```js
/// const total = Math.floor(ms / 1000);
/// const m = Math.floor(total / 60);
/// const s = total % 60;
/// return `${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
/// ```
///
/// `Math.floor` rounds toward negative infinity; JavaScript `%` keeps the sign
/// of the dividend; `padStart(2, '0')` only pads strings shorter than 2 chars
/// (so `-1` and any value `>= 10`/`<= -10` are left as-is). All three quirks are
/// reproduced so negative deltas format identically (`-1:00`, `-2:-59`, …).
#[must_use]
pub fn format_ms(ms: i64) -> String {
    let total = floor_div(ms, 1000);
    let m = floor_div(total, 60);
    let s = total % 60; // Rust `%` truncates toward zero == JS `%` for i64.
    format!("{}:{}", pad2(m), pad2(s))
}

/// Format a per-line stamp in `elapsed` mode: `formatMs(ts - (start || ts))`.
///
/// `start_ms == 0` reproduces the JS `(this.startTime || tsMs)` fallback — an
/// unknown/zero start makes the elapsed delta zero (`00:00`).
#[must_use]
pub fn format_elapsed(ts_ms: i64, start_ms: i64) -> String {
    // JS `(this.startTime || tsMs)`: 0 (and null/NaN) are falsy → fall back to ts.
    let start = if start_ms == 0 { ts_ms } else { start_ms };
    format_ms(ts_ms - start)
}

/// Format a per-line stamp in `ago` mode, byte-identical to the JS:
///
/// ```js
/// const sec = Math.max(0, Math.floor((Date.now() - tsMs) / 1000));
/// if (sec < 60) return `${sec}s ago`;
/// const m = Math.floor(sec / 60);
/// if (m < 60) return `${m}m ago`;
/// return `${Math.floor(m / 60)}h ago`;
/// ```
///
/// `now_ms` is the current time the orchestrator supplies (the core does not read
/// a clock). A future `ts` clamps to `0s ago`.
#[must_use]
pub fn format_ago(ts_ms: i64, now_ms: i64) -> String {
    let sec = floor_div(now_ms - ts_ms, 1000).max(0);
    if sec < 60 {
        return format!("{sec}s ago");
    }
    let m = floor_div(sec, 60);
    if m < 60 {
        return format!("{m}m ago");
    }
    format!("{}h ago", floor_div(m, 60))
}

/// Format a per-line stamp in `clock` mode: en-US 12-hour `h:mm AM/PM` from the
/// LOCAL clock components (`Date#getHours()` 0..=23 and `getMinutes()` 0..=59).
///
/// This reproduces `new Date(tsMs).toLocaleTimeString('en-US', { hour: 'numeric',
/// minute: '2-digit' })` without `Intl`: midnight and noon render the 12-hour
/// `12`, the hour is not zero-padded (`9:09 AM`), the minute is (`1:05 PM`). The
/// caller is responsible for the timezone conversion epoch-ms → local
/// `(hour, minute)`, which is the browser's job in `silent-web`.
///
/// `hour` outside `0..=23` and `minute` outside `0..=59` are clamped defensively
/// (a degenerate value is preferable to a panic on a render path); well-formed
/// inputs are unaffected.
#[must_use]
pub fn format_clock(hour: u8, minute: u8) -> String {
    let hour = hour.min(23);
    let minute = minute.min(59);
    let (h12, suffix) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        // 13..=23
        _ => (hour - 12, "PM"),
    };
    format!("{h12}:{minute:02} {suffix}")
}

/// History-list duration string, byte-identical to the JS `formatDuration`:
///
/// ```js
/// const total = Math.floor(ms / 1000);
/// const m = Math.floor(total / 60);
/// const s = total % 60;
/// return `${m}m ${s}s`;
/// ```
///
/// Unlike [`format_ms`] this is NOT zero-padded and uses the `Nm Ns` shape. It is
/// only ever called with a non-negative `duration` (end − start), so the sign
/// quirks of [`format_ms`] do not arise here; `i64` is used for a uniform
/// signature.
#[must_use]
pub fn format_duration(ms: i64) -> String {
    let total = floor_div(ms, 1000);
    let m = floor_div(total, 60);
    let s = total % 60;
    format!("{m}m {s}s")
}

// ---------------------------------------------------------------------------
// JS-arithmetic helpers.
// ---------------------------------------------------------------------------

/// `Math.floor(a / b)` for integers — floor division (rounds toward negative
/// infinity), unlike Rust's `/` which truncates toward zero. `b` is always a
/// positive literal (`1000`, `60`) at the call sites, so no zero/overflow guard
/// is needed.
fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    let r = a % b;
    // If the remainder is non-zero and the signs of a and b differ, the
    // truncated quotient is one too high — step it down to match `Math.floor`.
    if (r != 0) && ((r < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// `String(n).padStart(2, '0')`: left-pad to width 2 with `'0'`, but ONLY when
/// the decimal string is shorter than 2 characters. Negative numbers already
/// carry a `-` sign so e.g. `-1` (2 chars) and `-12` (3 chars) are NOT padded,
/// reproducing the JS exactly (`String(-1).padStart(2,'0') === '-1'`).
fn pad2(n: i64) -> String {
    let s = n.to_string();
    if s.len() >= 2 { s } else { format!("0{s}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_div_matches_math_floor() {
        // Positive
        assert_eq!(floor_div(0, 1000), 0);
        assert_eq!(floor_div(999, 1000), 0);
        assert_eq!(floor_div(1000, 1000), 1);
        // Negative (the JS Math.floor cases)
        assert_eq!(floor_div(-60_000, 1000), -60);
        assert_eq!(floor_div(-1, 1000), -1);
        assert_eq!(floor_div(-1000, 1000), -1);
        assert_eq!(floor_div(-60, 60), -1);
        assert_eq!(floor_div(-61, 60), -2);
    }

    #[test]
    fn pad2_only_pads_when_short() {
        assert_eq!(pad2(0), "00");
        assert_eq!(pad2(5), "05");
        assert_eq!(pad2(12), "12");
        assert_eq!(pad2(-1), "-1"); // already 2 chars: not padded (JS quirk)
        assert_eq!(pad2(-12), "-12");
    }

    #[test]
    fn cycle_order_and_wrap() {
        assert_eq!(TimestampMode::Elapsed.next(), TimestampMode::Clock);
        assert_eq!(TimestampMode::Clock.next(), TimestampMode::Ago);
        assert_eq!(TimestampMode::Ago.next(), TimestampMode::Elapsed); // wraps
    }

    #[test]
    fn labels_match_js_time_formats() {
        assert_eq!(TimestampMode::Elapsed.label(), "elapsed");
        assert_eq!(TimestampMode::Clock.label(), "clock");
        assert_eq!(TimestampMode::Ago.label(), "ago");
    }

    // Spot checks (the exhaustive table-driven equality lives in
    // tests/timestamp_golden.rs against the JS-generated fixtures).
    #[test]
    fn elapsed_negative_js_quirks() {
        assert_eq!(format_elapsed(5_000, 65_000), "-1:00");
        assert_eq!(format_elapsed(4_000, 65_000), "-2:-1");
        assert_eq!(format_elapsed(64_000, 65_000), "-1:-1");
    }

    #[test]
    fn clock_noon_and_midnight() {
        assert_eq!(format_clock(0, 0), "12:00 AM");
        assert_eq!(format_clock(12, 0), "12:00 PM");
        assert_eq!(format_clock(13, 23), "1:23 PM");
        assert_eq!(format_clock(9, 9), "9:09 AM");
    }
}
