/**
 * bridge-client.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * Role: a thin WebSocket client for the local Claude bridge (`bridge.py`) over
 * `ws://localhost:8765` — the user's own machine, inside the trust boundary
 * (PRD R5; hosted CSP keeps `ws://localhost:8765` in connect-src). It sends and
 * receives messages only; the reconnect / auto-backoff / status policy and the
 * transcript-batch / summary / screenshot-analysis logic are Rust policy
 * (Appendix A rows 27, 28). The bridge stays backend-agnostic (Claude CLI/API
 * now; Codex and other local agent CLIs are a deferred future option).
 *
 * # Executor vs policy split (PRD R2)
 *
 * This file is the EXECUTOR: it owns new WebSocket(), onopen/onclose/onmessage
 * wiring, and the `setTimeout` that arms the reconnect timer. What moved to Rust
 * (`bridge-engine.js` → `WasmBridgeReconnect`) is the POLICY:
 *   - whether to auto-reconnect (only after a prior successful connection)
 *   - after how long (5s → 10s → 20s → 40s, capped 60s)
 *   - what status the dot/label show
 *
 * # Constructor options
 *
 *   reconnectPolicy   Object with the bridgeReconnect facade interface:
 *                     { connectRequested, manualConnect, open, closed, setEnabled,
 *                       reset, isConnected, status, reconnectAttempt }
 *                     If not provided, a null-safe no-op facade is used (the
 *                     bridge connects but uses no Rust backoff policy).
 *   loadSettings      () => { claudeBridge: boolean, bridgeUrl: string, … }
 *                     Reads current user settings. Used in connect() to check
 *                     the enable toggle and read the URL.
 *   onMessage         (msg: object) => void
 *                     Called for every parsed inbound message. The message
 *                     dispatch (enhanced_notes, screenshot_analysis, etc.) is
 *                     the caller's responsibility — those callbacks touch DOM
 *                     and app state which live in index.html.
 *
 * # Phase 1 relocation
 *
 * The ClaudeBridge class was previously defined inline in index.html. It is now
 * exported from this module and dynamically imported by index.html. The class
 * interface (connect/disconnect/send/sendTranscript/sendScreenshot/requestSummary
 * /query/updateIndicator) is unchanged; `handleMessage` was extracted into the
 * caller-supplied `onMessage` callback to keep DOM/app logic out of this module.
 */

/** Null-safe reconnect policy returned when no policy is supplied. */
const NULL_POLICY = {
  connectRequested() { return { action: 'connect' }; },   // always connect (no policy)
  manualConnect()    { return { action: 'connect' }; },
  open()             { return { action: 'none' }; },
  closed()           { return { action: 'none' }; },       // no auto-reconnect without policy
  setEnabled()       {},
  reset()            {},
  isConnected()      { return false; },
  status()           { return 'disconnected'; },
  reconnectAttempt() { return 0; },
};

export class ClaudeBridge {
  /**
   * @param {object} [opts]
   * @param {object}   [opts.reconnectPolicy]  BridgeReconnect facade (Rust policy).
   * @param {Function} [opts.loadSettings]     () => settings object.
   * @param {Function} [opts.onMessage]        (msg) => void — inbound message handler.
   */
  constructor(opts = {}) {
    this._policy      = opts.reconnectPolicy || NULL_POLICY;
    this._loadSettings = opts.loadSettings   || (() => ({}));
    this.onMessage    = opts.onMessage       || null;

    this.ws             = null;
    this.connected      = false;
    this.reconnectTimer = null;
    this.callbacks      = {};
    const settings      = this._loadSettings();
    this.url            = settings.bridgeUrl || 'ws://localhost:8765';
  }

  /**
   * Swap in a loaded reconnect policy after async construction (used when the
   * Rust policy module finishes loading and replaces the null-safe facade).
   * @param {object} policy  BridgeReconnect instance.
   */
  setReconnectPolicy(policy) {
    this._policy = policy;
  }

  // Connect (or auto-reconnect). The Rust reconnect policy (Appendix A row 28)
  // owns the decision — whether to open a socket and, on close, whether/when to
  // retry. This executor just performs the action the policy returns and reports
  // lifecycle facts back.
  connect() {
    // Respect settings toggle. setEnabled keeps the policy in sync; a disabled
    // bridge yields {action:'none'} below (the old early-return path).
    const settings = this._loadSettings();
    this._policy.setEnabled(settings.claudeBridge !== false);
    const decision = this._policy.connectRequested();
    if (decision.action !== 'connect') {
      this.updateIndicator(false);
      return;
    }
    this._openSocket();
  }

  // Open the WebSocket and wire its handlers. The handlers report facts to the
  // Rust policy (open/closed) and act on the returned action (arm a backoff
  // timer for the policy-computed delay, or stop).
  _openSocket() {
    try {
      this.ws = new WebSocket(this.url);

      this.ws.onopen = () => {
        this.connected = true;
        this.updateIndicator(true);
        this._policy.open();   // clears backoff, records success
        this.send({ type: 'connect', timestamp: Date.now() });
        console.log('[ClaudeBridge] Connected');
        // Setup-panel feedback (only set when the user clicked "Connect").
        if (this._onPanelOpen) { const cb = this._onPanelOpen; this._onPanelOpen = null; this._onPanelClose = null; cb(); }
      };

      this.ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (this.onMessage) this.onMessage(msg);
        } catch (err) {
          console.error('[ClaudeBridge] Parse error:', err);
        }
      };

      this.ws.onclose = () => {
        this.connected = false;
        this.updateIndicator(false);
        // The Rust policy decides whether to auto-reconnect (only after a prior
        // successful connection — the old `if (wasConnected)` guard) and the
        // backoff delay (5s → 10s → 20s → 40s, capped 60s — replacing the flat
        // 5s). Console-logged so the backoff schedule is witnessable on real runs.
        const decision = this._policy.closed();
        if (decision.action === 'schedule_reconnect') {
          const delay = decision.delay_ms;
          console.log(`[ClaudeBridge] reconnect scheduled in ${delay}ms (attempt ${this._policy.reconnectAttempt()})`);
          this.reconnectTimer = setTimeout(() => this.connect(), delay);
        }
        // Setup-panel feedback (only set when the user clicked "Connect").
        if (this._onPanelClose) { const cb = this._onPanelClose; this._onPanelOpen = null; this._onPanelClose = null; cb(); }
      };

      this.ws.onerror = () => {
        // Will trigger onclose
      };
    } catch (err) {
      this.connected = false;
      this.updateIndicator(false);
      // A synchronous construction failure is also a "close" to the policy.
      this._policy.closed();
    }
  }

  disconnect() {
    if (this.reconnectTimer) { clearTimeout(this.reconnectTimer); this.reconnectTimer = null; }
    if (this.ws) { this.ws.close(); this.ws = null; }
    this.connected = false;
    // Reset the policy: a manual disconnect clears `wasConnected`, so the next
    // connect behaves like a first connect (no auto-reconnect until it succeeds).
    this._policy.reset();
    this.updateIndicator(false);
  }

  send(msg) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
      return true;
    }
    return false;
  }

  // Send transcript chunk for enhanced analysis
  sendTranscript(text, timestamp) {
    return this.send({ type: 'transcript_chunk', text, timestamp });
  }

  // Send screenshot for visual analysis
  sendScreenshot(imageBase64, timestamp) {
    return this.send({ type: 'screenshot', image_base64: imageBase64, timestamp });
  }

  // Request enhanced summary at end of meeting
  requestSummary(transcript, notes, screenshots) {
    return this.send({ type: 'generate_summary', transcript, notes, screenshots });
  }

  // Ad-hoc query during meeting
  query(question) {
    return this.send({ type: 'query', question });
  }

  updateIndicator(connected) {
    const dot   = document.getElementById('claudeDot');
    const label = document.getElementById('claudeLabel');
    if (dot)   { dot.className  = connected ? 'claude-dot connected' : 'claude-dot'; }
    if (label) { label.textContent = connected ? 'Claude connected' : 'Claude offline'; }
  }
}
