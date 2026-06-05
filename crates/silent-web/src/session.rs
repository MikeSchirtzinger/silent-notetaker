//! Wasm-bindgen recording-session surface (PRD Phase 4, Task h1 wiring;
//! Appendix A rows 1, 2, 3, 6, 31).
//!
//! Exposes the `silent-core` [`SessionMachine`] to the browser UI — the same
//! strangler-fig pattern as [`crate::diarization`] wraps `silent-diarization`
//! and [`crate::notes`] wraps `silent-notes`. The JS glue (`session-engine.js`)
//! loads the wasm-pack output (`pkg/`) and drives this object; it returns
//! serde-JSON values matching the typed boundary shapes
//! ([`silent_core::SessionEvent`], [`silent_core::session::SideEffect`]).
//!
//! # The law-vs-hands split (PRD R2)
//!
//! [`SessionMachine`] owns the *policy*: which transition is legal, cold-vs-warm
//! start (the resume-without-reload guarantee, Appendix A row 2), the Mic/Tab
//! source set (row 6), the 120-char title clamp (row 3), the timer projection
//! (row 1), and which stop-time passes fire (rows 15/19/21/31). This wrapper is
//! pure glue: every [`WasmSession`] method applies one [`UiCommand`] and returns
//! the resulting [`Outcome`] (events the UI renders + side effects the host
//! executes), JSON-serialized. No DOM, no clock, no I/O lives here or in the
//! machine — the host (`session-engine.js` + index.html) owns all of that.
//!
//! # Time is injected
//!
//! `silent-core` has no wall clock; every command that needs the time takes the
//! host's `Date.now()` as `now_ms`. The header timer is host-driven: the glue
//! keeps its 1 s `setInterval` and calls [`WasmSession::timer_text`] /
//! [`WasmSession::format_stamp`] / [`WasmSession::current_duration_str`] each
//! tick. In clock mode the host supplies the locale-formatted wall-clock string
//! (the machine has no `Intl`).
//!
//! # Auto-title (Appendix A row 3)
//!
//! `silent-core` cannot build the `"Wed, Jun 4 2:30 PM"` string (no locale). On
//! [`UiCommand::NewMeeting`] the machine resets the title to `"Untitled Meeting"`
//! and emits [`SessionEvent::TitleChanged`]; the host then computes its locale
//! date/time string and sends it back via [`WasmSession::set_title`]. The glue
//! wires `NewMeeting → (host computes auto-title) → set_title`.
//!
//! # wasm32-only
//!
//! Compiled only for `wasm32-unknown-unknown`; the native workspace build gates
//! this module out (see `lib.rs`), so `cargo check --workspace` stays browser-
//! dep-free.

use silent_core::commands::{SessionEvent, SessionState, UiCommand};
use silent_core::session::{Outcome, SessionConfig, SessionMachine};

use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Serialize a value to a `JsValue` via serde-json (a JSON string the glue
/// `JSON.parse`s). Matches the [`crate::diarization`] / [`crate::notes`]
/// convention so the whole `silent-web` boundary speaks one wire format.
fn to_js_value<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(v).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

/// Coerce a JS-number `now_ms` to the machine's `u64` clock. `Date.now()` is a
/// non-negative integer millisecond timestamp well within `u64`; clamp
/// defensively so a non-finite / negative input can never panic the boundary.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "now_ms is a Date.now() / session-elapsed millisecond timestamp \
              (finite, non-negative, far below u64::MAX); the guarded f64 → u64 \
              cast is exact for it. Non-finite / negative inputs are clamped to 0"
)]
fn ms(now_ms: f64) -> u64 {
    if now_ms.is_finite() && now_ms >= 0.0 {
        now_ms as u64
    } else {
        0
    }
}

/// The JSON envelope one [`Outcome`] serializes to for the glue: the events to
/// render and the side effects to execute, both already in
/// `{ "tag": ..., "payload": ... }` form (the `SessionEvent` / `SideEffect`
/// serde tagging). The glue iterates `events` then `effects` in order.
#[derive(serde::Serialize)]
struct OutcomeJson<'a> {
    events: &'a [SessionEvent],
    effects: &'a [silent_core::session::SideEffect],
}

fn outcome_to_js(outcome: &Outcome) -> Result<JsValue, JsError> {
    to_js_value(&OutcomeJson {
        events: &outcome.events,
        effects: &outcome.effects,
    })
}

// ---------------------------------------------------------------------------
// WasmSession — the recording-session state machine
// ---------------------------------------------------------------------------

/// Browser-facing recording-session surface: the deterministic
/// [`SessionMachine`] (Appendix A rows 1–3, 6, 24; stop-time hooks 15/19/21/31).
///
/// # Lifecycle (mirrors index.html `App.start/stop/newMeeting/tickTimer`)
///
/// Each user action drives ONE command; the returned outcome carries the UI
/// events and host side effects to process in order. The host never re-derives
/// cold-vs-warm or which stop-pass to run — it reads the side effect:
///
/// ```text
/// Start / Continue button → start(title, now)   // machine picks cold vs warm
///   cold  ⇒ effect "load_engine_and_capture"
///   warm  ⇒ effect "resume_capture_no_reload"   (Appendix A row 2: NO reload)
/// Stop button             → stop(now)
///   ⇒ effects "stop_capture_keep_engine_loaded" + "run_stop_hooks"{...}
///   ⇒ events  "state_changed", "sources_changed"{false,false}, "stop_hooks"{...}
/// New Meeting button      → new_meeting()
///   ⇒ event "title_changed"{"Untitled Meeting"} → host computes the locale
///     auto-title → set_title(autoTitle)          (Appendix A row 3)
/// title input change      → set_title(value)      (clamps to 120 chars)
/// Share/Remove Tab button → add_tab_audio(now) / remove_tab_audio(now)  (row 6)
/// timestamp cycle button  → cycle_timestamp_mode()
/// ```
///
/// Pure policy: no DOM, no model, no I/O. The DOM rendering, `getUserMedia`, the
/// engine load/resume, and the stop-time passes all stay in index.html /
/// `session-engine.js`; only the *decisions* moved into Rust.
#[wasm_bindgen]
pub struct WasmSession {
    machine: SessionMachine,
}

impl Default for WasmSession {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmSession {
    /// Create a fresh session machine in the `Idle` state with the default
    /// config (every stop-pass on except auto-summary, which is opt-in —
    /// matching index.html's `loadSettings()` defaults). Apply the user's actual
    /// settings with [`Self::set_config`] before the first Stop.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        Self {
            machine: SessionMachine::new(),
        }
    }

    /// Replace the stop-time configuration from the host's `loadSettings()`
    /// booleans (Appendix A rows 18–21, 31). `notes_model_off` is the
    /// transcript-only mode flag (`qwenModel === 'off'`, row 20) and is
    /// authoritative over the two model-driven passes. Does not change the
    /// current state — call it whenever settings change (the glue calls it on
    /// load and before each Stop).
    #[wasm_bindgen(js_name = setConfig)]
    #[allow(
        clippy::fn_params_excessive_bools,
        reason = "the five parameters are the five independent `loadSettings()` \
                  booleans SessionConfig documents 1:1; this is a thin pass-through \
                  to SessionConfig::new, which carries the same allow with the same \
                  rationale"
    )]
    pub fn set_config(
        &mut self,
        ai_final_notes: bool,
        smart_questions: bool,
        smartq_recap: bool,
        auto_summary: bool,
        notes_model_off: bool,
    ) {
        self.machine.set_config(SessionConfig::new(
            ai_final_notes,
            smart_questions,
            smartq_recap,
            auto_summary,
            notes_model_off,
        ));
    }

    /// Start (or Continue) a recording at `now_ms` (`App.start`). The machine
    /// decides cold-vs-warm from its loaded-engine flag, so the host uses ONE
    /// code path: read the side effect. From `Idle` this is a cold start
    /// (`load_engine_and_capture`); from `Stopped` with an engine loaded it is
    /// the warm Continue (`resume_capture_no_reload`, Appendix A row 2 — NO
    /// model reload). `title` is the live `#meetingTitle` value (a cold start
    /// resolves it: trim → default-if-empty → clamp; a warm Continue keeps the
    /// meeting's existing title).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure (cannot occur for
    /// these well-typed events/effects).
    pub fn start(&mut self, title: &str, now_ms: f64) -> Result<JsValue, JsError> {
        let out = self.machine.apply(
            &UiCommand::StartRecording {
                title: title.to_owned(),
            },
            ms(now_ms),
        );
        outcome_to_js(&out)
    }

    /// Explicit warm restart (`ResumeRecording`) — equivalent to Continue, but
    /// it never re-resolves the title. Valid only from `Stopped` with an engine
    /// loaded; otherwise a notice no-op. The Continue button uses [`Self::start`]
    /// (the machine picks warm automatically); this is provided for the path
    /// that wants the explicit resume semantics.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn resume(&mut self, now_ms: f64) -> Result<JsValue, JsError> {
        let out = self.machine.apply(&UiCommand::ResumeRecording, ms(now_ms));
        outcome_to_js(&out)
    }

    /// Stop the active recording at `now_ms` (`App.stop`). Freezes the final
    /// duration, keeps the engine loaded for Continue, clears the source badges,
    /// and emits the stop-time hooks: the `run_stop_hooks` side effect carries
    /// `{ recluster, final_notes, question_recap, auto_summary }` (the
    /// orchestrator's decision per `setConfig`). The summary *modal* always
    /// opens at Stop; `auto_summary` additionally requests the summary pass. A
    /// no-op if not currently recording.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn stop(&mut self, now_ms: f64) -> Result<JsValue, JsError> {
        let out = self.machine.apply(&UiCommand::StopRecording, ms(now_ms));
        outcome_to_js(&out)
    }

    /// Reset to a fresh meeting (`App.newMeeting`). Returns to `Idle`, drops the
    /// loaded-engine flag (a new meeting may pick a different engine), clears
    /// sources/timer/last-duration, and resets the title to `"Untitled Meeting"`
    /// — emitting [`SessionEvent::TitleChanged`] so the host installs the locale
    /// auto-title via [`Self::set_title`] (Appendix A row 3). Ignored while
    /// recording (the button is hidden then).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = newMeeting)]
    pub fn new_meeting(&mut self) -> Result<JsValue, JsError> {
        // now_ms is unused by NewMeeting; pass 0.
        let out = self.machine.apply(&UiCommand::NewMeeting, 0);
        outcome_to_js(&out)
    }

    /// Set/replace the pending meeting title (`SetTitle`). Clamps to 120 chars
    /// (Appendix A row 3, matching the `maxlength="120"` input) and echoes the
    /// clamped value back via [`SessionEvent::TitleChanged`] so the input
    /// reflects the cap. Used both for the title-input change handler and to
    /// install the host-computed auto-title after New Meeting.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = setTitle)]
    pub fn set_title(&mut self, title: &str) -> Result<JsValue, JsError> {
        let out = self.machine.apply(
            &UiCommand::SetTitle {
                title: title.to_owned(),
            },
            0,
        );
        outcome_to_js(&out)
    }

    /// Add tab/system audio to the active recording (`App.shareTab` on-path,
    /// Appendix A row 6). Only valid while Recording (else a notice no-op);
    /// idempotent. On success the `add_tab_audio` side effect tells the host to
    /// run `tm.addSystemAudio()`, and the `sources_changed` event raises the Tab
    /// Audio badge.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = addTabAudio)]
    pub fn add_tab_audio(&mut self, now_ms: f64) -> Result<JsValue, JsError> {
        let out = self.machine.apply(&UiCommand::AddTabAudio, ms(now_ms));
        outcome_to_js(&out)
    }

    /// Remove tab/system audio (`App.shareTab` off-path, or the shared stream
    /// ended via `onSystemAudioEnded`). Idempotent; lowers the Tab Audio badge.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = removeTabAudio)]
    pub fn remove_tab_audio(&mut self, now_ms: f64) -> Result<JsValue, JsError> {
        let out = self.machine.apply(&UiCommand::RemoveTabAudio, ms(now_ms));
        outcome_to_js(&out)
    }

    /// Cycle the timestamp display mode `elapsed → clock → ago` (`cycleTimeFormat`,
    /// Appendix A row 24). Emits [`SessionEvent::TimestampModeChanged`] with the
    /// new mode; the glue persists it and re-renders the stamps.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = cycleTimestampMode)]
    pub fn cycle_timestamp_mode(&mut self) -> Result<JsValue, JsError> {
        let out = self.machine.apply(&UiCommand::CycleTimestampMode, 0);
        outcome_to_js(&out)
    }

    // --- timer projection (host calls these every tick) -------------------

    /// The header timer string for the current state and mode (`App.tickTimer`).
    /// In clock mode pass the host's locale wall-clock string as `clock`; in
    /// elapsed/ago modes pass `null` and the machine returns `mm:ss` (live while
    /// recording, frozen at the last duration after Stop, `00:00` otherwise).
    #[wasm_bindgen(js_name = timerText)]
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen exported methods take Option<String> by value (the \
                  generated glue moves the optional string across the boundary; \
                  Option<&str> is not a supported wasm-bindgen argument type); \
                  the body borrows it via as_deref()"
    )]
    pub fn timer_text(&self, now_ms: f64, clock: Option<String>) -> String {
        self.machine.timer_text(ms(now_ms), clock.as_deref())
    }

    /// The export duration string — always `mm:ss`, independent of the header's
    /// display mode (`App.currentDurationStr`).
    #[wasm_bindgen(js_name = currentDurationStr)]
    #[must_use]
    pub fn current_duration_str(&self, now_ms: f64) -> String {
        self.machine.current_duration_str(ms(now_ms))
    }

    /// Format a per-line timestamp (`ts_ms`, epoch) for the active mode
    /// (`App.formatStamp`). In clock mode pass the host's locale string as
    /// `clock`; in ago mode the machine computes `now - ts`; in elapsed mode
    /// `ts - start`.
    #[wasm_bindgen(js_name = formatStamp)]
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen exported methods take Option<String> by value (the \
                  generated glue moves the optional string across the boundary; \
                  Option<&str> is not a supported wasm-bindgen argument type); \
                  the body borrows it via as_deref()"
    )]
    pub fn format_stamp(&self, ts_ms: f64, now_ms: f64, clock: Option<String>) -> String {
        self.machine
            .format_stamp(ms(ts_ms), ms(now_ms), clock.as_deref())
    }

    // --- read-only state accessors (the glue mirrors DOM from these) ------

    /// The active (clamped) meeting title.
    #[wasm_bindgen(js_name = title)]
    #[must_use]
    pub fn title(&self) -> String {
        self.machine.title().to_owned()
    }

    /// The active timestamp display-mode key (`"elapsed"`/`"clock"`/`"ago"`),
    /// for the format button label and the `timeFormat` persisted setting.
    #[wasm_bindgen(js_name = timestampMode)]
    #[must_use]
    pub fn timestamp_mode(&self) -> String {
        self.machine.timestamp_mode().as_str().to_owned()
    }

    /// The externally-visible state key (`"idle"`/`"loading"`/`"recording"`/
    /// `"stopped"`). The glue uses it for assertions / smoke checks; button
    /// visibility is driven by the `state_changed` events, not polled.
    #[wasm_bindgen(js_name = state)]
    #[must_use]
    pub fn state(&self) -> String {
        let key = match self.machine.state() {
            SessionState::Loading => "loading",
            SessionState::Recording => "recording",
            SessionState::Stopped => "stopped",
            // `Idle` plus the #[non_exhaustive] catch-all: an unknown future
            // state maps to "idle" (the pre-recording default) rather than
            // panicking the boundary.
            _ => "idle",
        };
        key.to_owned()
    }

    /// Whether an engine is loaded (so the next start resumes warm). True from
    /// the first successful start until New Meeting — the witness for the
    /// resume-without-reload guarantee (Appendix A row 2).
    #[wasm_bindgen(js_name = engineLoaded)]
    #[must_use]
    pub fn engine_loaded(&self) -> bool {
        self.machine.engine_loaded()
    }

    /// Whether mic capture is active (true throughout Recording).
    #[wasm_bindgen(js_name = micActive)]
    #[must_use]
    pub fn mic_active(&self) -> bool {
        self.machine.mic_active()
    }

    /// Whether tab/system audio is mixed in.
    #[wasm_bindgen(js_name = tabActive)]
    #[must_use]
    pub fn tab_active(&self) -> bool {
        self.machine.tab_active()
    }
}
