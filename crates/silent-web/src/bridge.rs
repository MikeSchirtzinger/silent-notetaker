//! Wasm-bindgen Claude-bridge reconnect surface (PRD Phase 4, Appendix A
//! row 28).
//!
//! Exposes the `silent-core` [`ReconnectPolicy`] to the browser UI — the same
//! strangler-fig pattern as [`crate::session`] wraps the recording-session
//! machine. The JS glue (`bridge-engine.js`) loads the wasm-pack output (`pkg/`)
//! and drives this object; the inline `ClaudeBridge` executor keeps the
//! WebSocket (per the PRD: the socket stays in JS). Each method records one
//! lifecycle fact and returns the [`Action`](silent_core::bridge::Action) the
//! executor must take, JSON-
//! serialized to match the `silent_core::bridge::Action` boundary shape.
//!
//! # Law vs. hands (PRD R2)
//!
//! [`ReconnectPolicy`] owns the *policy*: the connection [`BridgeStatus`], the
//! exponential backoff schedule (`5s → 10s → 20s → 40s`, capped at `60s`), and
//! the "only auto-reconnect after a prior successful connection" rule. This
//! wrapper is pure glue: no socket, no timer, no clock lives here. The executor
//! opens the `WebSocket`, arms the `setTimeout`, and renders the dot/label — it
//! just asks this object *what* to do.
//!
//! # wasm32-only
//!
//! Compiled only for `wasm32-unknown-unknown`; the native workspace build gates
//! this module out (see `lib.rs`), so `cargo check --workspace` stays browser-
//! dep-free.

use silent_core::bridge::{BridgeStatus, ReconnectPolicy};

use wasm_bindgen::prelude::*;

/// Serialize a value to a `JsValue` via serde-json (a JSON string the glue
/// `JSON.parse`s). Matches the [`crate::session`] / [`crate::diarization`]
/// convention so the whole `silent-web` boundary speaks one wire format.
fn to_js_value<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(v).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(JsValue::from_str(&s))
}

/// Browser-facing Claude-bridge reconnect surface: the deterministic
/// [`ReconnectPolicy`] (Appendix A row 28).
///
/// # Lifecycle (mirrors the inline `ClaudeBridge` executor)
///
/// The JS executor owns the socket and reports facts; this object decides:
///
/// ```text
/// connect()  /  reconnect timer fires → connectRequested()  → "connect" | "none"
/// setup panel "Connect" button         → manualConnect()     → "connect" | "none"
/// ws.onopen                             → open()              → "none" (dot lights)
/// ws.onclose / ws.onerror→onclose       → closed()            → "schedule_reconnect"{delay_ms} | "none"
/// settings.claudeBridge toggle          → setEnabled(bool)
/// disconnect() / teardown               → reset()
/// ```
///
/// On an [`Action::ScheduleReconnect`](silent_core::bridge::Action::ScheduleReconnect)
/// the executor arms `setTimeout(…, delay_ms)`
/// and calls [`Self::connect_requested`] again when it elapses — the policy
/// advances the backoff each `closed()` and resets it on a successful `open()`.
#[wasm_bindgen]
pub struct WasmBridgeReconnect {
    policy: ReconnectPolicy,
}

impl Default for WasmBridgeReconnect {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmBridgeReconnect {
    /// Create a fresh reconnect policy: disconnected, never-connected, enabled —
    /// matching a just-constructed `ClaudeBridge` before its first `connect()`.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        Self {
            policy: ReconnectPolicy::new(),
        }
    }

    /// A connection attempt is being made now (the executor's `connect()` body or
    /// an armed reconnect timer firing). Returns the JSON action: `"connect"` to
    /// open a socket, or `"none"` if the bridge is disabled.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure (cannot occur for
    /// this well-typed action).
    #[wasm_bindgen(js_name = connectRequested)]
    pub fn connect_requested(&mut self) -> Result<JsValue, JsError> {
        to_js_value(&self.policy.connect_requested())
    }

    /// A manual connect from the setup panel's "Connect" button
    /// (`retryBridgeConnect`). Resets the backoff counter and always opens
    /// (regardless of whether the bridge ever connected). `"connect"` unless
    /// disabled.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = manualConnect)]
    pub fn manual_connect(&mut self) -> Result<JsValue, JsError> {
        to_js_value(&self.policy.manual_connect())
    }

    /// The socket's `onopen` fired — the bridge is live. Clears the backoff,
    /// records the success (arming future auto-reconnect), lights the dot.
    /// Always returns `"none"`.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn open(&mut self) -> Result<JsValue, JsError> {
        to_js_value(&self.policy.open())
    }

    /// The socket's `onclose` fired. Returns `"schedule_reconnect"{delay_ms}` if a
    /// connection had previously succeeded (arm `setTimeout(delay_ms)` then call
    /// [`Self::connect_requested`]); otherwise `"none"` (the server was never
    /// running — show offline and wait for a manual Connect).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn closed(&mut self) -> Result<JsValue, JsError> {
        to_js_value(&self.policy.closed())
    }

    /// Apply the user's settings toggle (`settings.claudeBridge`). A disabled
    /// bridge does not connect or auto-reconnect.
    #[wasm_bindgen(js_name = setEnabled)]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.policy.set_enabled(enabled);
    }

    /// Reset to the fresh, never-connected state (manual disconnect / teardown).
    /// The next connect behaves like a first connect (no auto-reconnect until it
    /// succeeds).
    pub fn reset(&mut self) {
        self.policy.reset();
    }

    // --- read-only accessors (the glue renders the dot/label from these) ----

    /// Whether the status dot should be lit (`status == connected`).
    #[wasm_bindgen(js_name = isConnected)]
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.policy.is_connected()
    }

    /// The current status key (`"disconnected"`/`"connecting"`/`"connected"`/
    /// `"reconnecting"`) — the optional setup-panel detail line, and the witness
    /// assertion target.
    #[wasm_bindgen(js_name = status)]
    #[must_use]
    pub fn status(&self) -> String {
        let s: BridgeStatus = self.policy.status();
        s.as_str().to_owned()
    }

    /// The consecutive-reconnect attempt counter (0 while connected / freshly
    /// reset). Exposed so the witness/console can confirm the backoff advanced.
    #[wasm_bindgen(js_name = reconnectAttempt)]
    #[must_use]
    pub fn reconnect_attempt(&self) -> u32 {
        self.policy.reconnect_attempt()
    }
}
