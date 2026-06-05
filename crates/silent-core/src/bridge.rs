//! The Claude-bridge reconnect/backoff policy (PRD Phase 4, Appendix A row 28).
//!
//! Row 28 is the local Claude bridge: a WebSocket client (`ws://localhost:8765`)
//! with auto-reconnect/backoff, a status dot, a setup panel, and an inline
//! `bridge.py` download. Per the PRD, the WebSocket itself stays in JS
//! (`bridge-client.js` / the inline `ClaudeBridge` executor) — it owns the
//! `new WebSocket()`, the `onopen`/`onclose`/`onmessage` handlers, and the
//! `setTimeout` that arms the next attempt. What moves into Rust here is the
//! *policy*: **should** we reconnect, **after how long**, and **what status**
//! the dot/label should show. That decision is now a deterministic state
//! machine with unit tests instead of an ad-hoc `setTimeout(…, 5000)` buried in
//! a closure.
//!
//! # Law vs. hands (PRD R2)
//!
//! [`ReconnectPolicy`] is the law: it owns the connection [`BridgeStatus`], the
//! exponential backoff schedule, and the "only auto-reconnect after a prior
//! successful connection" rule (so a bridge that was never running does not get
//! spammed — the exact behavior of the JS `if (wasConnected)` guard it
//! replaces). The JS executor is the hands: it reports lifecycle facts
//! ([`open`](ReconnectPolicy::open), [`closed`](ReconnectPolicy::closed),
//! [`connect_requested`](ReconnectPolicy::connect_requested)) and acts on the
//! returned [`Action`] (open a socket, or arm a timer for `delay_ms`).
//!
//! # Determinism
//!
//! No clock, no I/O, no async, no randomness. The backoff delays are a fixed
//! schedule (`5s → 10s → 20s → 40s`, capped at `60s`) so a given sequence of
//! lifecycle events always yields the same actions and the same status — which
//! is what makes the schedule and the status transitions testable without a
//! browser or a live bridge. The host supplies wall-time only as the duration it
//! sleeps before re-driving the policy; the policy never reads a clock itself.
//!
//! # Privacy
//!
//! This module carries connection *state* only — never transcript text,
//! screenshots, audio, or any meeting content. Those flow over the socket the JS
//! executor owns; the policy decides *whether the socket should exist*, nothing
//! about what crosses it.

use serde::{Deserialize, Serialize};

/// The user-visible connection status of the Claude bridge.
///
/// The status dot (`#claudeDot`) and label (`#claudeLabel`) render from this:
/// only [`Connected`](BridgeStatus::Connected) lights the dot and shows "Claude
/// connected"; every other status shows the offline dot and "Claude offline" —
/// pixel-identical to the JS `updateIndicator(connected)` which only ever
/// distinguished connected-vs-not. The richer non-connected variants exist so
/// the policy (and tests, and a future setup-panel detail line) can reason about
/// *why* it is not connected, without changing what the dot shows today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeStatus {
    /// No socket and no pending reconnect — the idle/offline resting state
    /// (page load before the first attempt, or after a give-up with no
    /// auto-reconnect armed).
    Disconnected,
    /// A socket is opening (`new WebSocket()` issued, `onopen` not yet fired).
    Connecting,
    /// `onopen` fired — the bridge is live. The only status that lights the dot.
    Connected,
    /// The socket closed after a successful connection and a reconnect is armed;
    /// the host is sleeping `delay_ms` before the next [`Action::Connect`]. The
    /// dot is offline during this window (matching the JS, which set the dot off
    /// in `onclose` and only relit it on the next `onopen`).
    Reconnecting,
}

impl BridgeStatus {
    /// Whether the status dot should be lit / the label should read
    /// "Claude connected" (the single bit the JS `updateIndicator` consumed).
    #[must_use]
    pub fn is_connected(self) -> bool {
        matches!(self, BridgeStatus::Connected)
    }

    /// The stable string key for the status (for the glue's optional detail line
    /// and for assertions in the witness).
    ///
    /// The match is exhaustive within this crate (all variants are known here);
    /// the `#[non_exhaustive]` attribute only forces a wildcard arm on *external*
    /// crates. A future variant added here MUST be given a key — the compiler
    /// enforces that, which is the safer choice than a silent catch-all.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BridgeStatus::Disconnected => "disconnected",
            BridgeStatus::Connecting => "connecting",
            BridgeStatus::Connected => "connected",
            BridgeStatus::Reconnecting => "reconnecting",
        }
    }
}

/// The action the JS executor must take in response to a lifecycle event.
///
/// The policy never touches the socket; it returns one of these and the
/// executor performs it. `None` means "do nothing" (idempotent events, or a
/// close with no auto-reconnect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, rename = "BridgeAction"))]
#[serde(tag = "action", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Action {
    /// Open a socket now (`new WebSocket(url)`). Issued for a manual connect and
    /// when an armed reconnect timer should fire immediately (the executor calls
    /// [`ReconnectPolicy::connect_requested`] after its `setTimeout` elapses, and
    /// gets this).
    Connect,
    /// Arm a reconnect timer for `delay_ms`, then re-drive the policy by calling
    /// [`ReconnectPolicy::connect_requested`] when it elapses. Issued after a
    /// close that follows a prior successful connection.
    ScheduleReconnect {
        /// The backoff delay before the next attempt, in milliseconds.
        delay_ms: u32,
    },
    /// Do nothing (an already-handled or non-reconnecting event).
    None,
}

/// The exponential-backoff reconnect schedule, in milliseconds.
///
/// Index `n` is the delay before the `n`-th consecutive reconnect attempt after
/// a connection was lost. The first delay (`5000`) matches the JS's flat
/// `setTimeout(…, 5000)`; subsequent attempts back off (`10s → 20s → 40s`) and
/// then hold at the [`MAX_BACKOFF_MS`] cap, so a bridge that stays down does not
/// get hammered every 5 s for the life of the page. A successful `open` resets
/// the attempt counter to `0`, so the *next* outage starts again at `5s`.
pub const BACKOFF_SCHEDULE_MS: [u32; 4] = [5_000, 10_000, 20_000, 40_000];

/// The maximum backoff delay (the cap once [`BACKOFF_SCHEDULE_MS`] is
/// exhausted). Every attempt past the schedule waits this long.
pub const MAX_BACKOFF_MS: u32 = 60_000;

/// Look up the backoff delay for the `attempt`-th consecutive reconnect
/// (0-indexed: `attempt == 0` is the first reconnect after an outage).
#[must_use]
pub fn backoff_delay_ms(attempt: u32) -> u32 {
    let idx = attempt as usize;
    if idx < BACKOFF_SCHEDULE_MS.len() {
        BACKOFF_SCHEDULE_MS[idx]
    } else {
        MAX_BACKOFF_MS
    }
}

/// The deterministic Claude-bridge reconnect/backoff policy.
///
/// A small state machine the JS WebSocket executor drives. It holds the current
/// [`BridgeStatus`], whether a connection ever succeeded (`ever_connected` — the
/// `wasConnected` guard), and the consecutive-reconnect attempt counter (for the
/// backoff schedule). It performs no I/O: each method records a lifecycle fact
/// and returns the [`Action`] the executor should take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectPolicy {
    status: BridgeStatus,
    /// Whether the bridge has connected at least once since the last manual
    /// reset. Mirrors the JS `wasConnected` flag the `onclose` handler checked
    /// before arming a reconnect: a bridge that was never running is not
    /// auto-retried (the user starts it and clicks "Connect").
    had_successful_connection: bool,
    /// Number of consecutive failed/lost connections since the last successful
    /// `open` — the index into [`BACKOFF_SCHEDULE_MS`].
    reconnect_attempt: u32,
    /// Whether the bridge is enabled by the user's settings toggle
    /// (`settings.claudeBridge`). When disabled, [`connect_requested`] is a
    /// no-op (the JS `connect()` early-returns and shows the dot offline).
    enabled: bool,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconnectPolicy {
    /// A fresh policy: disconnected, never-connected, enabled. Matches a
    /// just-constructed `ClaudeBridge` before its first `connect()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            status: BridgeStatus::Disconnected,
            had_successful_connection: false,
            reconnect_attempt: 0,
            enabled: true,
        }
    }

    /// The current status (what the dot/label render from).
    #[must_use]
    pub fn status(&self) -> BridgeStatus {
        self.status
    }

    /// Whether the dot should be lit (`status == Connected`).
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.status.is_connected()
    }

    /// The consecutive-reconnect attempt counter (0 while connected or freshly
    /// reset). Exposed for the witness / diagnostics.
    #[must_use]
    pub fn reconnect_attempt(&self) -> u32 {
        self.reconnect_attempt
    }

    /// Whether the bridge has ever connected since the last reset (the
    /// `wasConnected` guard).
    #[must_use]
    pub fn had_successful_connection(&self) -> bool {
        self.had_successful_connection
    }

    /// Apply the user's settings toggle (`settings.claudeBridge`). Disabling does
    /// not itself tear down a live socket (the executor's `disconnect()` does
    /// that); it makes the next [`connect_requested`] a no-op and is reflected in
    /// the status when the executor disconnects.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether the bridge is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// A connection attempt is being made now (the JS `connect()` body or an
    /// armed reconnect timer firing). Returns the action to take:
    ///
    /// - If the bridge is disabled, [`Action::None`] (the JS early-return path)
    ///   and the status goes to `Disconnected`.
    /// - Otherwise the status goes to `Connecting` and the executor opens a
    ///   socket ([`Action::Connect`]).
    ///
    /// This is called both for the initial auto-connect on page load and after a
    /// `setTimeout` reconnect elapses — the executor opens the socket either way.
    pub fn connect_requested(&mut self) -> Action {
        if !self.enabled {
            self.status = BridgeStatus::Disconnected;
            return Action::None;
        }
        self.status = BridgeStatus::Connecting;
        Action::Connect
    }

    /// A manual connect from the setup panel's "Connect" button
    /// (`retryBridgeConnect`). Always tries fresh: it resets the backoff attempt
    /// counter (the user explicitly asked, so the next outage backs off from the
    /// start) and opens a socket regardless of `had_successful_connection`. A
    /// disabled bridge still yields [`Action::None`].
    pub fn manual_connect(&mut self) -> Action {
        self.reconnect_attempt = 0;
        self.connect_requested()
    }

    /// The socket's `onopen` fired — the bridge is live. Clears the backoff
    /// counter, records the successful connection (arming future auto-reconnect),
    /// and moves to [`BridgeStatus::Connected`]. The dot lights.
    ///
    /// Returns [`Action::None`] — opening is terminal for the executor's open
    /// path (it just renders the connected status).
    pub fn open(&mut self) -> Action {
        self.status = BridgeStatus::Connected;
        self.had_successful_connection = true;
        self.reconnect_attempt = 0;
        Action::None
    }

    /// The socket's `onclose` (or `onerror`→`onclose`) fired. Returns the
    /// reconnect decision:
    ///
    /// - If a connection had previously succeeded (`wasConnected`), arm the next
    ///   attempt at the current backoff delay ([`Action::ScheduleReconnect`]),
    ///   advance the attempt counter, and move to [`BridgeStatus::Reconnecting`].
    /// - If the bridge never connected (server not running), do **not**
    ///   auto-reconnect ([`Action::None`]); the status goes to `Disconnected`.
    ///   The user starts `bridge.py` and clicks "Connect" (a [`manual_connect`]).
    ///
    /// This is the exact policy the JS `onclose` closure encoded inline; it now
    /// lives here with a real schedule and tests.
    ///
    /// [`manual_connect`]: ReconnectPolicy::manual_connect
    pub fn closed(&mut self) -> Action {
        let was_connected = self.status.is_connected() || self.had_successful_connection;
        if was_connected && self.had_successful_connection && self.enabled {
            let delay_ms = backoff_delay_ms(self.reconnect_attempt);
            self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
            self.status = BridgeStatus::Reconnecting;
            Action::ScheduleReconnect { delay_ms }
        } else {
            self.status = BridgeStatus::Disconnected;
            Action::None
        }
    }

    /// Reset to the fresh, never-connected state (a manual disconnect from the
    /// panel, or a full teardown). The next `connect()` behaves like a first
    /// connect — no auto-reconnect until it succeeds.
    pub fn reset(&mut self) {
        self.status = BridgeStatus::Disconnected;
        self.had_successful_connection = false;
        self.reconnect_attempt = 0;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism (PRD lint config)"
)]
mod tests {
    use super::*;

    #[test]
    fn fresh_policy_is_disconnected_and_never_connected() {
        let p = ReconnectPolicy::new();
        assert_eq!(p.status(), BridgeStatus::Disconnected);
        assert!(!p.is_connected());
        assert!(!p.had_successful_connection());
        assert_eq!(p.reconnect_attempt(), 0);
    }

    #[test]
    fn initial_connect_then_open_lights_the_dot() {
        let mut p = ReconnectPolicy::new();
        assert_eq!(p.connect_requested(), Action::Connect);
        assert_eq!(p.status(), BridgeStatus::Connecting);
        assert!(!p.is_connected());

        assert_eq!(p.open(), Action::None);
        assert_eq!(p.status(), BridgeStatus::Connected);
        assert!(p.is_connected());
        assert!(p.had_successful_connection());
    }

    #[test]
    fn close_before_any_success_does_not_auto_reconnect() {
        // Server never running: connect → close (no open) → no reconnect, just
        // offline. Mirrors the JS `if (wasConnected)` guard exactly.
        let mut p = ReconnectPolicy::new();
        assert_eq!(p.connect_requested(), Action::Connect);
        assert_eq!(p.closed(), Action::None);
        assert_eq!(p.status(), BridgeStatus::Disconnected);
        assert_eq!(p.reconnect_attempt(), 0);
    }

    #[test]
    fn close_after_success_schedules_reconnect_at_first_backoff() {
        let mut p = ReconnectPolicy::new();
        p.connect_requested();
        p.open();
        let action = p.closed();
        assert_eq!(action, Action::ScheduleReconnect { delay_ms: 5_000 });
        assert_eq!(p.status(), BridgeStatus::Reconnecting);
        assert!(!p.is_connected());
    }

    #[test]
    fn backoff_schedule_escalates_then_caps() {
        // Connect once, then a run of failed reconnects: 5s, 10s, 20s, 40s, then
        // hold at the 60s cap. This is the deterministic schedule the witness
        // observes in the console.
        let mut p = ReconnectPolicy::new();
        p.connect_requested();
        p.open();

        let mut delays = Vec::new();
        // first close after success:
        if let Action::ScheduleReconnect { delay_ms } = p.closed() {
            delays.push(delay_ms);
        }
        // each subsequent attempt: timer fires → connect_requested → socket
        // fails to open → closed() again. `had_successful_connection` stays true
        // (it only clears on reset), so reconnects keep being armed.
        for _ in 0..6 {
            assert_eq!(p.connect_requested(), Action::Connect);
            assert_eq!(p.status(), BridgeStatus::Connecting);
            if let Action::ScheduleReconnect { delay_ms } = p.closed() {
                delays.push(delay_ms);
            }
        }
        assert_eq!(
            delays,
            vec![5_000, 10_000, 20_000, 40_000, 60_000, 60_000, 60_000]
        );
    }

    #[test]
    fn successful_reconnect_resets_the_backoff() {
        let mut p = ReconnectPolicy::new();
        p.connect_requested();
        p.open();
        // outage: two failed attempts (5s, then 10s)
        assert_eq!(p.closed(), Action::ScheduleReconnect { delay_ms: 5_000 });
        p.connect_requested();
        assert_eq!(p.closed(), Action::ScheduleReconnect { delay_ms: 10_000 });
        // now the bridge comes back: open() clears the counter
        p.connect_requested();
        p.open();
        assert_eq!(p.reconnect_attempt(), 0);
        // the NEXT outage starts again at 5s
        assert_eq!(p.closed(), Action::ScheduleReconnect { delay_ms: 5_000 });
    }

    #[test]
    fn manual_connect_resets_backoff_and_always_opens() {
        let mut p = ReconnectPolicy::new();
        // never connected — auto path would not reconnect, but the user clicks
        // Connect and we always try.
        assert_eq!(p.manual_connect(), Action::Connect);
        assert_eq!(p.status(), BridgeStatus::Connecting);

        // simulate an outage that advanced the counter, then a manual retry:
        p.open();
        p.closed(); // attempt -> 1
        p.connect_requested();
        p.closed(); // attempt -> 2
        assert_eq!(p.reconnect_attempt(), 2);
        assert_eq!(p.manual_connect(), Action::Connect);
        assert_eq!(p.reconnect_attempt(), 0);
    }

    #[test]
    fn disabled_bridge_does_not_connect_or_reconnect() {
        let mut p = ReconnectPolicy::new();
        p.set_enabled(false);
        assert_eq!(p.connect_requested(), Action::None);
        assert_eq!(p.status(), BridgeStatus::Disconnected);

        // even after a prior success, a disabled bridge does not auto-reconnect.
        p.set_enabled(true);
        p.connect_requested();
        p.open();
        p.set_enabled(false);
        assert_eq!(p.closed(), Action::None);
        assert_eq!(p.status(), BridgeStatus::Disconnected);
    }

    #[test]
    fn reset_clears_was_connected_so_next_close_is_offline_only() {
        let mut p = ReconnectPolicy::new();
        p.connect_requested();
        p.open();
        p.reset();
        assert!(!p.had_successful_connection());
        assert_eq!(p.status(), BridgeStatus::Disconnected);
        // after reset, a connect that closes without opening does NOT reconnect.
        p.connect_requested();
        assert_eq!(p.closed(), Action::None);
    }

    #[test]
    fn status_connected_is_the_only_lit_state() {
        assert!(BridgeStatus::Connected.is_connected());
        for s in [
            BridgeStatus::Disconnected,
            BridgeStatus::Connecting,
            BridgeStatus::Reconnecting,
        ] {
            assert!(!s.is_connected(), "{} must be offline", s.as_str());
        }
    }

    #[test]
    fn action_serializes_tagged() {
        let json = serde_json::to_string(&Action::ScheduleReconnect { delay_ms: 5_000 })
            .expect("serialize");
        assert!(
            json.contains("\"action\":\"schedule_reconnect\""),
            "tagged: {json}"
        );
        assert!(json.contains("\"delay_ms\":5000"), "carries delay: {json}");
        let back: Action = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Action::ScheduleReconnect { delay_ms: 5_000 });
    }

    #[test]
    fn backoff_lookup_matches_schedule_and_cap() {
        assert_eq!(backoff_delay_ms(0), 5_000);
        assert_eq!(backoff_delay_ms(1), 10_000);
        assert_eq!(backoff_delay_ms(2), 20_000);
        assert_eq!(backoff_delay_ms(3), 40_000);
        assert_eq!(backoff_delay_ms(4), MAX_BACKOFF_MS);
        assert_eq!(backoff_delay_ms(99), MAX_BACKOFF_MS);
    }
}
