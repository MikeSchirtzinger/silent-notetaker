//! The recording-session state machine (PRD Phase 4, Appendix A rows 1–3, 6,
//! 24, and the stop-time trigger points 15/19/21/31).
//!
//! This is the orchestrator's heart: the deterministic, browser-free state
//! machine that owns start / stop / resume-without-reload / new-meeting, the
//! Mic / Tab source set, the meeting timer, the 120-char auto-title, and the
//! typed stop-time hooks. It is the Rust "law" for the behavior that lives today
//! in index.html's `App.start()` / `App.stop()` / `App.newMeeting()` /
//! `App.tickTimer()` / `App.updateTabUI()` (anchors in Appendix A).
//!
//! # Why a pure state machine
//!
//! Per PRD R2 the *policy* (which transition is legal, when to reload a model,
//! which stop-time passes fire) is Rust; the *execution* (actually loading the
//! engine, opening `getUserMedia`, running the Qwen recap) is the host. So
//! [`SessionMachine`] takes [`UiCommand`]s and returns [`SessionEvent`]s plus a
//! small set of [`SideEffect`]s the host must perform — and contains **no**
//! clock, no I/O, no async. That is exactly what makes every transition
//! deterministically testable without a browser (PRD "Validation plan").
//!
//! # Time is injected, never read
//!
//! `silent-core` has no wall clock (it must compile for
//! `wasm32-unknown-unknown` with no `web-sys`/`js-sys`). The host supplies the
//! current epoch-millis on the commands that need it (start, stop, tick) and
//! the machine returns formatted strings. This keeps the timer logic — the
//! `formatMs` / `formatStamp` ports — pure and golden-testable.

use serde::{Deserialize, Serialize};

use crate::commands::{SessionEvent, SessionState, StopHooks, TimestampMode, UiCommand};

/// The default meeting title used when the user clears the input, matching
/// index.html's `value.trim() || 'Untitled Meeting'` at start.
pub const DEFAULT_TITLE: &str = "Untitled Meeting";

/// Maximum meeting-title length (Appendix A row 3, the `maxlength="120"` input).
///
/// Counted in Unicode scalar values (`char`s). HTML `maxlength` counts UTF-16
/// code units, so a title built entirely of astral-plane characters could in
/// principle differ; for the BMP text real titles use, char-count and
/// UTF-16-unit-count agree, and clamping by `char` can never split a scalar
/// value (which truncating bytes could). This is the safe, deterministic
/// choice and is covered by [`clamp_title`]'s tests.
pub const MAX_TITLE_CHARS: usize = 120;

/// Configuration that gates the stop-time passes (Appendix A rows 18–21, 31).
///
/// These mirror the `loadSettings()` booleans index.html reads inside `stop()`.
/// Defaults match the app's defaults: every pass on except where the user opts
/// out (`!== false` in JS means "on unless explicitly disabled").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent JS `loadSettings()` boolean ported \
              1:1; a bitflags/sub-struct would obscure the direct correspondence \
              to the behavior being ported"
)]
pub struct SessionConfig {
    /// `aiFinalNotes !== false` — run the Qwen final-notes pass at Stop.
    pub ai_final_notes: bool,
    /// `smartQuestions !== false` — the smart-questions feature is enabled.
    pub smart_questions: bool,
    /// `smartqRecap !== false` — run the stop-time question recap (gated by
    /// `smart_questions` too).
    pub smartq_recap: bool,
    /// `autoSummary` — request the on-device/bridge summary pass at Stop
    /// (Appendix A row 31). The summary *modal* always opens regardless.
    pub auto_summary: bool,
    /// `qwenModel === 'off'` — the notes/questions model slot is **empty**
    /// (transcript-only mode, PRD R3 / Appendix A row 20). This is a first-class
    /// `NotesSlot=None` state, not a degraded one: when set, *no* notes-model
    /// pass may run. It is authoritative over [`Self::ai_final_notes`] and
    /// [`Self::smart_questions`] in [`SessionMachine::stop_hooks`], because those
    /// passes have no model to execute against. The live regex trigger notes
    /// (Appendix A row 16) are model-free and keep working — they are owned by
    /// `silent-notes` `NoteExtractor`, not gated here.
    pub notes_model_off: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        // "on unless explicitly disabled" for the `!== false` flags; auto-summary
        // is opt-in (its localStorage key is absent by default), matching the app.
        Self {
            ai_final_notes: true,
            smart_questions: true,
            smartq_recap: true,
            auto_summary: false,
            // The notes model defaults to present (`qwenModel: 'auto'`); the
            // empty-slot transcript-only mode is an explicit opt-in.
            notes_model_off: false,
        }
    }
}

/// A side effect the host must perform as a result of a transition.
///
/// The machine never performs I/O; it *describes* the work the host owns
/// (loading the engine, opening capture, running stop-time passes). This is the
/// command-half of the R2 split and keeps the machine pure. Distinct from
/// [`SessionEvent`]: events are for the UI to render, side effects are for the
/// host runtime to execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SideEffect {
    /// Cold start: load the selected engine, then open mic capture and begin
    /// transcription. Emitted on the first start of a meeting (index.html's
    /// `!canResume` branch).
    LoadEngineAndCapture,

    /// Warm restart: the engine is already loaded; re-open capture and resume
    /// the streaming loop **without** reloading the model (Appendix A row 2,
    /// index.html's `canResume` branch).
    ResumeCaptureNoReload,

    /// Tear down capture and finalize the engine's trailing tail (index.html's
    /// `tm.stop()` + awaiting `_nemotronStopPromise`). The engine stays loaded
    /// so a subsequent resume is warm.
    StopCaptureKeepEngineLoaded,

    /// Begin mixing tab/system audio into the live capture (`addSystemAudio`).
    AddTabAudio,

    /// Stop mixing tab/system audio (`removeSystemAudio`).
    RemoveTabAudio,

    /// Run the stop-time passes the orchestrator decided to fire. Carries the
    /// same [`StopHooks`] flags emitted to the UI so the host has the decision
    /// without re-deriving it.
    RunStopHooks(StopHooks),
}

/// The recording-session state machine.
///
/// Drive it with [`SessionMachine::apply`]. It is a plain value (no I/O, no
/// clock) so it is trivially `Clone`, serializable for command-log replay, and
/// deterministically testable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionMachine {
    state: SessionState,
    /// Whether an engine is loaded in the host. Set by a successful start,
    /// stays true across Stop (models are kept for Continue/New Meeting),
    /// cleared by New Meeting. This is what makes resume-without-reload a typed
    /// guarantee rather than a host-side guess.
    engine_loaded: bool,
    /// The pending/active meeting title (already clamped to [`MAX_TITLE_CHARS`]).
    title: String,
    /// Mic capture is active (true throughout Recording; false otherwise).
    mic_active: bool,
    /// Tab/system audio is mixed in.
    tab_active: bool,
    /// Epoch-millis at which the active recording started, or `None` when not
    /// recording. Drives the elapsed timer.
    start_time_ms: Option<u64>,
    /// Duration of the *last completed* recording, milliseconds. Lets the timer
    /// freeze on the final value after Stop (index.html's `_lastDuration`).
    last_duration_ms: Option<u64>,
    /// The active timestamp display mode (Appendix A row 24).
    timestamp_mode: TimestampMode,
    /// Stop-time pass gating (Appendix A rows 18–21, 31).
    config: SessionConfig,
}

/// The outcome of applying one [`UiCommand`]: the events to forward to the UI
/// and the side effects for the host to execute, in order.
///
/// An empty outcome means the command was a no-op in the current state (for
/// example `StopRecording` while already `Stopped`) — never a panic. Invalid
/// transitions are *ignored with a [`SessionEvent::Notice`]*, matching the
/// app's behavior of disabled buttons rather than hard errors.
#[derive(Debug, Clone, PartialEq, Default)]
#[must_use = "the host must forward events to the UI and run the side effects"]
pub struct Outcome {
    /// Events to forward to the UI, in order.
    pub events: Vec<SessionEvent>,
    /// Side effects for the host to execute, in order.
    pub effects: Vec<SideEffect>,
}

impl Outcome {
    fn new() -> Self {
        Self::default()
    }

    fn event(mut self, e: SessionEvent) -> Self {
        self.events.push(e);
        self
    }

    fn effect(mut self, e: SideEffect) -> Self {
        self.effects.push(e);
        self
    }

    /// A no-op outcome carrying only an explanatory notice (an ignored command
    /// in the current state).
    fn notice(message: impl Into<String>) -> Self {
        Self::new().event(SessionEvent::Notice {
            message: message.into(),
        })
    }
}

impl Default for SessionMachine {
    fn default() -> Self {
        Self {
            state: SessionState::Idle,
            engine_loaded: false,
            title: DEFAULT_TITLE.to_owned(),
            mic_active: false,
            tab_active: false,
            start_time_ms: None,
            last_duration_ms: None,
            timestamp_mode: TimestampMode::default(),
            config: SessionConfig::default(),
        }
    }
}

impl SessionMachine {
    /// A fresh machine in [`SessionState::Idle`] with the default title and
    /// config.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a machine with a non-default stop-time configuration.
    #[must_use]
    pub fn with_config(config: SessionConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// The current externally-visible state.
    #[must_use]
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Whether an engine is currently loaded in the host (so the next start can
    /// resume warm). True from the first successful start until New Meeting.
    #[must_use]
    pub fn engine_loaded(&self) -> bool {
        self.engine_loaded
    }

    /// The active (clamped) meeting title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The active timestamp display mode.
    #[must_use]
    pub fn timestamp_mode(&self) -> TimestampMode {
        self.timestamp_mode
    }

    /// Whether mic capture is active.
    #[must_use]
    pub fn mic_active(&self) -> bool {
        self.mic_active
    }

    /// Whether tab/system audio is mixed in.
    #[must_use]
    pub fn tab_active(&self) -> bool {
        self.tab_active
    }

    /// Replace the stop-time configuration (the user changed a setting between
    /// recordings). Does not affect the current state.
    pub fn set_config(&mut self, config: SessionConfig) {
        self.config = config;
    }

    /// Apply one [`UiCommand`], advancing the machine and returning the events
    /// and side effects the host must process.
    ///
    /// `now_ms` is the host's current epoch-millis, used to stamp recording
    /// start/stop and compute durations. It is ignored by commands that do not
    /// need a clock (rename, engine selection, timestamp cycling, …) so callers
    /// may pass any value there.
    pub fn apply(&mut self, command: &UiCommand, now_ms: u64) -> Outcome {
        match command {
            UiCommand::StartRecording { title } => self.on_start(Some(title), now_ms),
            UiCommand::ResumeRecording => self.on_resume(now_ms),
            UiCommand::StopRecording => self.on_stop(now_ms),
            UiCommand::NewMeeting => self.on_new_meeting(),
            UiCommand::SetTitle { title } => self.on_set_title(title),
            UiCommand::AddTabAudio => self.on_tab_audio(true),
            UiCommand::RemoveTabAudio => self.on_tab_audio(false),
            UiCommand::CycleTimestampMode => self.on_cycle_timestamp(),
            // Engine/notes selection and speaker rename are owned by other
            // subsystems (silent-inference / silent-diarization); the session
            // machine ignores them. The wildcard also future-proofs against
            // additive `#[non_exhaustive]` variants without a compile break.
            _ => Outcome::new(),
        }
    }

    // --- transitions ------------------------------------------------------

    /// `StartRecording`. From `Idle` this is a cold start (load engine);
    /// from `Stopped` it is the "Continue" warm path (no reload). From
    /// `Recording`/`Loading` it is ignored (the button is disabled in the app).
    fn on_start(&mut self, title: Option<&str>, now_ms: u64) -> Outcome {
        match self.state {
            SessionState::Idle | SessionState::Stopped => {
                let warm = self.engine_loaded;
                // Apply the title exactly like index.html, which reads the live
                // `#meetingTitle` input at start: a non-empty trimmed value wins;
                // an empty start value falls back to the pending title set via
                // `SetTitle` (still in the input); if that too is empty/default,
                // the `'Untitled Meeting'` default applies. Only a *cold* start
                // re-resolves the title — a warm "Continue" keeps the meeting's
                // existing title.
                if let (false, Some(t)) = (warm, title) {
                    let trimmed = t.trim();
                    self.title = if trimmed.is_empty() {
                        // empty start input ⇒ keep any pending SetTitle value,
                        // else the default.
                        resolve_title(&self.title)
                    } else {
                        resolve_title(trimmed)
                    };
                }
                self.begin_recording(now_ms, warm)
            }
            // Already recording / loading: disabled-button no-op.
            _ => Outcome::new(),
        }
    }

    /// `ResumeRecording` — the explicit warm restart (Appendix A row 2). Only
    /// valid from `Stopped` with an engine loaded; otherwise a notice no-op.
    fn on_resume(&mut self, now_ms: u64) -> Outcome {
        match self.state {
            SessionState::Stopped if self.engine_loaded => self.begin_recording(now_ms, true),
            SessionState::Stopped => {
                // Stopped but the engine was dropped — cannot resume warm.
                Outcome::notice("No loaded engine to resume; start a new recording")
            }
            _ => Outcome::notice("Nothing to resume"),
        }
    }

    /// Shared start body for cold start, Continue, and Resume. `warm` selects
    /// the reload-vs-no-reload side effect (the resume-without-reload guarantee).
    fn begin_recording(&mut self, now_ms: u64, warm: bool) -> Outcome {
        self.state = SessionState::Recording;
        self.engine_loaded = true;
        self.mic_active = true;
        self.tab_active = false; // tab is re-added per recording, never carried
        self.start_time_ms = Some(now_ms);

        let effect = if warm {
            SideEffect::ResumeCaptureNoReload
        } else {
            SideEffect::LoadEngineAndCapture
        };

        Outcome::new()
            .event(SessionEvent::StateChanged {
                state: SessionState::Recording,
            })
            .event(SessionEvent::SourcesChanged {
                mic: true,
                tab: false,
            })
            .effect(effect)
    }

    /// `StopRecording`. Computes the stop-time hooks, freezes the timer, keeps
    /// the engine loaded for Continue, and clears the source badges. Ignored
    /// (no-op) if not currently recording.
    fn on_stop(&mut self, now_ms: u64) -> Outcome {
        if self.state != SessionState::Recording {
            return Outcome::new();
        }

        // Freeze the final duration (index.html's `_lastDuration`). `now_ms`
        // is assumed monotonic w.r.t. start; a clock that went backwards
        // saturates to zero rather than underflowing.
        self.last_duration_ms = self.start_time_ms.map(|start| now_ms.saturating_sub(start));
        self.start_time_ms = None;

        let had_tab = self.tab_active;
        self.state = SessionState::Stopped;
        self.mic_active = false;
        self.tab_active = false;
        // engine_loaded stays true — Continue/New Meeting decide its fate.

        let hooks = self.stop_hooks();

        let mut out = Outcome::new()
            .event(SessionEvent::StateChanged {
                state: SessionState::Stopped,
            })
            .effect(SideEffect::StopCaptureKeepEngineLoaded);

        // Mirror index.html: if tab audio was active it is torn down at Stop.
        if had_tab {
            out = out.effect(SideEffect::RemoveTabAudio);
        }

        out.event(SessionEvent::SourcesChanged {
            mic: false,
            tab: false,
        })
        .event(SessionEvent::StopHooks(hooks))
        .effect(SideEffect::RunStopHooks(hooks))
    }

    /// Compute which stop-time passes fire, mirroring the guards in
    /// index.html's `stop()`. `recluster` cannot be decided from session state
    /// alone (it depends on the diarization speaker count); the orchestrator
    /// requests it whenever diarization is in play, and `silent-diarization`
    /// no-ops when there is `<= 1` speaker. Here we encode the *policy* default:
    /// request recluster, run final-notes/recap per config, always offer the
    /// summary pass per `auto_summary`.
    ///
    /// When [`SessionConfig::notes_model_off`] is set (transcript-only mode,
    /// PRD R3 / Appendix A row 20) the empty notes slot is authoritative: the
    /// two model-driven passes (`final_notes`, `question_recap`) cannot fire —
    /// there is no model to run — regardless of their per-pass flags. `recluster`
    /// (diarization) and `auto_summary` (bridge/on-device summary) are unrelated
    /// to the notes model and are unaffected.
    fn stop_hooks(&self) -> StopHooks {
        let notes_model = !self.config.notes_model_off;
        StopHooks {
            recluster: true,
            final_notes: notes_model && self.config.ai_final_notes,
            question_recap: notes_model && self.config.smart_questions && self.config.smartq_recap,
            auto_summary: self.config.auto_summary,
        }
    }

    /// `NewMeeting` — full reset to `Idle` with a fresh auto-title slot. Drops
    /// the loaded-engine flag (a new meeting may pick a different engine), and
    /// clears sources, timer, and last-duration. The auto date/time title is
    /// computed by the host (it owns the clock/locale) and applied via a
    /// following [`UiCommand::SetTitle`]; here we reset to the default so the UI
    /// never shows a stale title. Valid from any state that is not actively
    /// recording (the button is hidden during Recording).
    fn on_new_meeting(&mut self) -> Outcome {
        if self.state == SessionState::Recording || self.state == SessionState::Loading {
            // The New Meeting button is hidden while recording — ignore.
            return Outcome::new();
        }

        *self = Self::with_config(self.config);

        Outcome::new()
            .event(SessionEvent::StateChanged {
                state: SessionState::Idle,
            })
            .event(SessionEvent::TitleChanged {
                title: self.title.clone(),
            })
            .event(SessionEvent::SourcesChanged {
                mic: false,
                tab: false,
            })
    }

    /// `SetTitle` — clamp and stash the pending title. Allowed in any state but
    /// only meaningful before/between recordings (the UI input is editable
    /// then). Always emits the clamped value back so the UI input reflects the
    /// 120-char limit.
    fn on_set_title(&mut self, title: &str) -> Outcome {
        self.title = clamp_title(title);
        Outcome::new().event(SessionEvent::TitleChanged {
            title: self.title.clone(),
        })
    }

    /// `AddTabAudio` / `RemoveTabAudio` — only valid while recording (the
    /// Share/Remove Tab button is shown only then). Idempotent: adding when
    /// already on, or removing when already off, is a no-op.
    fn on_tab_audio(&mut self, add: bool) -> Outcome {
        if self.state != SessionState::Recording {
            return Outcome::notice("Start recording before adding tab audio");
        }
        if self.tab_active == add {
            return Outcome::new(); // already in the requested state
        }
        self.tab_active = add;
        let effect = if add {
            SideEffect::AddTabAudio
        } else {
            SideEffect::RemoveTabAudio
        };
        Outcome::new()
            .effect(effect)
            .event(SessionEvent::SourcesChanged {
                mic: self.mic_active,
                tab: self.tab_active,
            })
    }

    /// `CycleTimestampMode` — rotate `elapsed → clock → ago` (Appendix A row
    /// 24). Allowed in any state (the cycle button is always live).
    fn on_cycle_timestamp(&mut self) -> Outcome {
        self.timestamp_mode = self.timestamp_mode.next();
        Outcome::new().event(SessionEvent::TimestampModeChanged {
            mode: self.timestamp_mode,
        })
    }

    // --- timer projection (pure formatters) -------------------------------

    /// The header timer string for the current state and mode, given the host's
    /// current epoch-millis (Appendix A row 1/24). Ports index.html's
    /// `tickTimer()`:
    ///
    /// - In [`TimestampMode::Clock`], always wall-clock `h:mm AM/PM`, even
    ///   before recording starts.
    /// - Otherwise, `mm:ss` elapsed since start; `00:00` when not started, and
    ///   the frozen last duration after Stop.
    ///
    /// In clock mode the host must supply `clock` (it owns the locale); pass
    /// `None` and the machine falls back to the elapsed projection so a caller
    /// without a formatted clock string still gets a sensible value.
    #[must_use]
    pub fn timer_text(&self, now_ms: u64, clock: Option<&str>) -> String {
        // In clock mode use the host-supplied wall-clock string; if the host
        // gave none, fall through to the elapsed projection below.
        if self.timestamp_mode == TimestampMode::Clock
            && let Some(c) = clock
        {
            return c.to_owned();
        }
        match self.start_time_ms {
            Some(start) => format_ms(now_ms.saturating_sub(start)),
            None => match self.last_duration_ms {
                Some(d) => format_ms(d),
                None => format_ms(0),
            },
        }
    }

    /// The duration string for exports — always `mm:ss`, independent of the
    /// header's display mode (index.html's `currentDurationStr`). Live while
    /// recording, the frozen last duration after Stop, `00:00` otherwise.
    #[must_use]
    pub fn current_duration_str(&self, now_ms: u64) -> String {
        match self.start_time_ms {
            Some(start) => format_ms(now_ms.saturating_sub(start)),
            None => match self.last_duration_ms {
                Some(d) => format_ms(d),
                None => format_ms(0),
            },
        }
    }

    /// Elapsed milliseconds since recording start, or `None` if not recording.
    #[must_use]
    pub fn elapsed_ms(&self, now_ms: u64) -> Option<u64> {
        self.start_time_ms.map(|s| now_ms.saturating_sub(s))
    }

    /// Format a per-line timestamp (`ts_ms`, epoch) per the active mode,
    /// porting index.html's `formatStamp`. In `clock` mode the host supplies the
    /// wall-clock string (locale-owned); in `ago` mode the relative string is
    /// computed from `now_ms - ts_ms`; in `elapsed` mode it is `ts_ms - start`.
    #[must_use]
    pub fn format_stamp(&self, ts_ms: u64, now_ms: u64, clock: Option<&str>) -> String {
        match self.timestamp_mode {
            TimestampMode::Clock => clock.map_or_else(|| format_ms(0), str::to_owned),
            TimestampMode::Ago => format_ago(now_ms.saturating_sub(ts_ms)),
            // `elapsed` (and any future variant) → elapsed since start.
            _ => {
                let start = self.start_time_ms.unwrap_or(ts_ms);
                format_ms(ts_ms.saturating_sub(start))
            }
        }
    }
}

// --- free functions: pure ports of the JS formatters ---------------------

/// Clamp a title to [`MAX_TITLE_CHARS`] Unicode scalar values, trimming nothing
/// (the input is taken verbatim; trimming/defaulting happens in [`resolve_title`]
/// at start). Mirrors the `maxlength="120"` input cap.
#[must_use]
pub fn clamp_title(title: &str) -> String {
    title.chars().take(MAX_TITLE_CHARS).collect()
}

/// Resolve the effective title at recording start: trim, fall back to the
/// default if empty, then clamp — exactly index.html's
/// `value.trim() || 'Untitled Meeting'` followed by the input cap.
#[must_use]
pub fn resolve_title(raw: &str) -> String {
    let trimmed = raw.trim();
    let base = if trimmed.is_empty() {
        DEFAULT_TITLE
    } else {
        trimmed
    };
    clamp_title(base)
}

/// `mm:ss` from milliseconds, zero-padded — a byte-exact port of index.html's
/// `formatMs`: integer seconds, minutes can exceed 99 (no hours rollover).
#[must_use]
pub fn format_ms(ms: u64) -> String {
    let total = ms / 1000;
    let m = total / 60;
    let s = total % 60;
    format!("{m:02}:{s:02}")
}

/// Relative "ago" string from an elapsed-millis delta — a port of the `'ago'`
/// branch of index.html's `formatStamp`: `<60s` → `"Ns ago"`, `<60m` →
/// `"Nm ago"`, else `"Nh ago"`.
#[must_use]
pub fn format_ago(delta_ms: u64) -> String {
    let sec = delta_ms / 1000;
    if sec < 60 {
        return format!("{sec}s ago");
    }
    let m = sec / 60;
    if m < 60 {
        return format!("{m}m ago");
    }
    format!("{}h ago", m / 60)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism (PRD lint config \
              allows this in tests)"
)]
mod tests {
    use super::*;
    use crate::commands::SessionEvent;

    // Convenience: drive a machine and return the outcome (asserted on).
    // Takes the command by value purely for call-site ergonomics — every caller
    // constructs the `UiCommand` inline — and borrows it for the real
    // `&UiCommand` API; hence the narrow needless-pass-by-value allowance.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "test ergonomics: callers build the command inline; the by-value \
                  receiver avoids `&` at ~50 call sites"
    )]
    fn apply(m: &mut SessionMachine, c: UiCommand, now: u64) -> Outcome {
        m.apply(&c, now)
    }

    // Setup-only driver: advances the machine and discards the outcome (used
    // where a test only cares about the *resulting* state, not that step's
    // events/effects). Keeps the `#[must_use]` on `Outcome` honest at the API
    // while keeping setup terse.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "test ergonomics, as for `apply` above"
    )]
    fn drive(m: &mut SessionMachine, c: UiCommand, now: u64) {
        let _ = m.apply(&c, now);
    }

    fn has_state_change(out: &Outcome, want: SessionState) -> bool {
        out.events
            .iter()
            .any(|e| matches!(e, SessionEvent::StateChanged { state } if *state == want))
    }

    // -------- title / formatter spot-checks --------
    //
    // These document the intended behavior inline and run fast. The *provenance*
    // — that every expected string came out of the actual index.html JS — is
    // proven separately and exhaustively by `tests/session_golden.rs`, which
    // replays the JSON fixture emitted by `goldens/gen/session_ref.mjs` (a
    // DOM-free port of `formatMs` / `formatStamp` / the title trim+clamp). Both
    // layers must agree; the golden test is the byte-for-byte contract.

    #[test]
    fn format_ms_spot_check() {
        // Spot-check of index.html `formatMs`: mm:ss, no hour rollover. The
        // full JS-generated table lives in tests/session_golden.rs.
        assert_eq!(format_ms(0), "00:00");
        assert_eq!(format_ms(999), "00:00"); // sub-second floors
        assert_eq!(format_ms(1_000), "00:01");
        assert_eq!(format_ms(59_000), "00:59");
        assert_eq!(format_ms(60_000), "01:00");
        assert_eq!(format_ms(61_500), "01:01"); // floors the .5s
        assert_eq!(format_ms(600_000), "10:00");
        assert_eq!(format_ms(6_000_000), "100:00"); // minutes exceed 99, no hours
    }

    #[test]
    fn format_ago_spot_check() {
        // Spot-check of the index.html `formatStamp` 'ago' branch; the full
        // JS-generated table lives in tests/session_golden.rs.
        assert_eq!(format_ago(0), "0s ago");
        assert_eq!(format_ago(30_000), "30s ago");
        assert_eq!(format_ago(59_000), "59s ago");
        assert_eq!(format_ago(60_000), "1m ago");
        assert_eq!(format_ago(59 * 60_000), "59m ago");
        assert_eq!(format_ago(60 * 60_000), "1h ago");
        assert_eq!(format_ago(150 * 60_000), "2h ago"); // 2.5h floors
    }

    #[test]
    fn clamp_title_caps_at_120_chars() {
        let long = "x".repeat(200);
        assert_eq!(clamp_title(&long).chars().count(), MAX_TITLE_CHARS);
        let short = "Standup";
        assert_eq!(clamp_title(short), "Standup");
        // Clamping by char never splits a multi-byte scalar value.
        let emoji = "😀".repeat(130);
        let clamped = clamp_title(&emoji);
        assert_eq!(clamped.chars().count(), MAX_TITLE_CHARS);
        assert!(clamped.chars().all(|c| c == '😀'));
    }

    #[test]
    fn resolve_title_trims_defaults_and_clamps() {
        // `value.trim() || 'Untitled Meeting'`
        assert_eq!(resolve_title("   "), DEFAULT_TITLE);
        assert_eq!(resolve_title(""), DEFAULT_TITLE);
        assert_eq!(resolve_title("  Weekly Sync  "), "Weekly Sync");
        // trim then clamp: 130 chars of content → 120
        let padded = format!("  {}  ", "y".repeat(130));
        assert_eq!(resolve_title(&padded).chars().count(), MAX_TITLE_CHARS);
    }

    // -------- timestamp-mode cycle --------

    #[test]
    fn timestamp_mode_cycles_elapsed_clock_ago() {
        assert_eq!(TimestampMode::Elapsed.next(), TimestampMode::Clock);
        assert_eq!(TimestampMode::Clock.next(), TimestampMode::Ago);
        assert_eq!(TimestampMode::Ago.next(), TimestampMode::Elapsed);
        assert_eq!(TimestampMode::default(), TimestampMode::Elapsed);
    }

    #[test]
    fn cycle_timestamp_emits_mode_and_persists() {
        let mut m = SessionMachine::new();
        let out = apply(&mut m, UiCommand::CycleTimestampMode, 0);
        assert_eq!(m.timestamp_mode(), TimestampMode::Clock);
        assert!(matches!(
            out.events.as_slice(),
            [SessionEvent::TimestampModeChanged {
                mode: TimestampMode::Clock
            }]
        ));
        drive(&mut m, UiCommand::CycleTimestampMode, 0);
        drive(&mut m, UiCommand::CycleTimestampMode, 0);
        assert_eq!(m.timestamp_mode(), TimestampMode::Elapsed); // wrapped
    }

    // -------- the core transition matrix --------

    #[test]
    fn idle_start_is_a_cold_load() {
        let mut m = SessionMachine::new();
        assert_eq!(m.state(), SessionState::Idle);
        assert!(!m.engine_loaded());

        let out = apply(
            &mut m,
            UiCommand::StartRecording {
                title: "  Planning  ".into(),
            },
            1_000,
        );

        assert_eq!(m.state(), SessionState::Recording);
        assert!(m.engine_loaded());
        assert_eq!(m.title(), "Planning"); // trimmed
        assert!(m.mic_active() && !m.tab_active());
        // Cold start ⇒ LoadEngineAndCapture, NOT a warm resume.
        assert_eq!(out.effects, vec![SideEffect::LoadEngineAndCapture]);
        assert!(has_state_change(&out, SessionState::Recording));
        assert!(out.events.contains(&SessionEvent::SourcesChanged {
            mic: true,
            tab: false
        }));
    }

    #[test]
    fn empty_title_falls_back_to_default_at_start() {
        let mut m = SessionMachine::new();
        drive(
            &mut m,
            UiCommand::StartRecording {
                title: "   ".into(),
            },
            0,
        );
        assert_eq!(m.title(), DEFAULT_TITLE);
    }

    #[test]
    fn stop_after_recording_keeps_engine_and_emits_hooks() {
        let mut m = SessionMachine::new();
        drive(
            &mut m,
            UiCommand::StartRecording { title: "M".into() },
            10_000,
        );
        let out = apply(&mut m, UiCommand::StopRecording, 70_000);

        assert_eq!(m.state(), SessionState::Stopped);
        assert!(m.engine_loaded()); // kept for Continue
        assert!(!m.mic_active() && !m.tab_active());
        // 60s recording froze as the last duration.
        assert_eq!(m.current_duration_str(0), "01:00");

        // StopHooks event + RunStopHooks effect both present, both default-on.
        let hooks = StopHooks {
            recluster: true,
            final_notes: true,
            question_recap: true,
            auto_summary: false,
        };
        assert!(out.events.contains(&SessionEvent::StopHooks(hooks)));
        assert!(out.effects.contains(&SideEffect::RunStopHooks(hooks)));
        assert!(
            out.effects
                .contains(&SideEffect::StopCaptureKeepEngineLoaded)
        );
        assert!(has_state_change(&out, SessionState::Stopped));
    }

    #[test]
    fn continue_after_stop_resumes_without_reload() {
        let mut m = SessionMachine::new();
        drive(
            &mut m,
            UiCommand::StartRecording {
                title: "Sync".into(),
            },
            0,
        );
        drive(&mut m, UiCommand::StopRecording, 5_000);

        // "Continue" = StartRecording from Stopped with an engine loaded.
        let out = apply(
            &mut m,
            UiCommand::StartRecording {
                title: "ignored on warm".into(),
            },
            6_000,
        );
        assert_eq!(m.state(), SessionState::Recording);
        // THE resume-without-reload guarantee:
        assert_eq!(out.effects, vec![SideEffect::ResumeCaptureNoReload]);
        assert!(!out.effects.contains(&SideEffect::LoadEngineAndCapture));
        // Warm continue keeps the original title.
        assert_eq!(m.title(), "Sync");
    }

    #[test]
    fn explicit_resume_command_is_warm() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "A".into() }, 0);
        drive(&mut m, UiCommand::StopRecording, 1_000);
        let out = apply(&mut m, UiCommand::ResumeRecording, 2_000);
        assert_eq!(m.state(), SessionState::Recording);
        assert_eq!(out.effects, vec![SideEffect::ResumeCaptureNoReload]);
    }

    #[test]
    fn resume_from_idle_is_a_notice_noop() {
        let mut m = SessionMachine::new();
        let out = apply(&mut m, UiCommand::ResumeRecording, 0);
        assert_eq!(m.state(), SessionState::Idle);
        assert!(out.effects.is_empty());
        assert!(matches!(
            out.events.as_slice(),
            [SessionEvent::Notice { .. }]
        ));
    }

    #[test]
    fn new_meeting_after_stop_resets_to_idle_and_drops_engine() {
        let mut m = SessionMachine::new();
        drive(
            &mut m,
            UiCommand::StartRecording {
                title: "Kept until reset".into(),
            },
            0,
        );
        drive(&mut m, UiCommand::StopRecording, 1_000);
        assert!(m.engine_loaded());

        let out = apply(&mut m, UiCommand::NewMeeting, 2_000);
        assert_eq!(m.state(), SessionState::Idle);
        assert!(!m.engine_loaded()); // a fresh meeting may pick a new engine
        assert_eq!(m.title(), DEFAULT_TITLE); // re-armed
        assert!(!m.mic_active() && !m.tab_active());
        assert!(has_state_change(&out, SessionState::Idle));
        assert!(
            out.events
                .iter()
                .any(|e| matches!(e, SessionEvent::TitleChanged { .. }))
        );
    }

    #[test]
    fn new_meeting_during_recording_is_ignored() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        let out = apply(&mut m, UiCommand::NewMeeting, 1_000);
        assert_eq!(m.state(), SessionState::Recording); // unchanged
        assert!(out.events.is_empty() && out.effects.is_empty());
    }

    #[test]
    fn double_start_while_recording_is_ignored() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        let out = apply(&mut m, UiCommand::StartRecording { title: "y".into() }, 1);
        assert_eq!(m.state(), SessionState::Recording);
        assert!(out.events.is_empty() && out.effects.is_empty());
        assert_eq!(m.title(), "x"); // second start did not overwrite
    }

    #[test]
    fn stop_while_idle_is_a_noop() {
        let mut m = SessionMachine::new();
        let out = apply(&mut m, UiCommand::StopRecording, 0);
        assert_eq!(m.state(), SessionState::Idle);
        assert!(out.events.is_empty() && out.effects.is_empty());
    }

    #[test]
    fn stop_while_already_stopped_is_a_noop() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        drive(&mut m, UiCommand::StopRecording, 1_000);
        let out = apply(&mut m, UiCommand::StopRecording, 2_000);
        assert!(out.events.is_empty() && out.effects.is_empty());
    }

    // -------- source tracking (Mic / Tab badges) --------

    #[test]
    fn tab_audio_add_remove_during_recording() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);

        let add = apply(&mut m, UiCommand::AddTabAudio, 1);
        assert!(m.tab_active());
        assert_eq!(add.effects, vec![SideEffect::AddTabAudio]);
        assert!(add.events.contains(&SessionEvent::SourcesChanged {
            mic: true,
            tab: true
        }));

        // Idempotent add: no-op.
        let again = apply(&mut m, UiCommand::AddTabAudio, 2);
        assert!(again.events.is_empty() && again.effects.is_empty());

        let rem = apply(&mut m, UiCommand::RemoveTabAudio, 3);
        assert!(!m.tab_active());
        assert_eq!(rem.effects, vec![SideEffect::RemoveTabAudio]);
        assert!(rem.events.contains(&SessionEvent::SourcesChanged {
            mic: true,
            tab: false
        }));
    }

    #[test]
    fn tab_audio_before_recording_is_a_notice_noop() {
        let mut m = SessionMachine::new();
        let out = apply(&mut m, UiCommand::AddTabAudio, 0);
        assert!(!m.tab_active());
        assert!(matches!(
            out.events.as_slice(),
            [SessionEvent::Notice { .. }]
        ));
    }

    #[test]
    fn stop_with_tab_active_tears_down_tab() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        drive(&mut m, UiCommand::AddTabAudio, 1);
        let out = apply(&mut m, UiCommand::StopRecording, 2_000);
        assert!(out.effects.contains(&SideEffect::RemoveTabAudio));
        assert!(!m.tab_active());
    }

    #[test]
    fn tab_does_not_carry_across_a_resume() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        drive(&mut m, UiCommand::AddTabAudio, 1);
        drive(&mut m, UiCommand::StopRecording, 2_000);
        drive(&mut m, UiCommand::ResumeRecording, 3_000);
        // Resume re-arms with mic only; tab must be re-shared explicitly.
        assert!(m.mic_active() && !m.tab_active());
    }

    // -------- set-title between recordings --------

    #[test]
    fn set_title_clamps_and_echoes() {
        let mut m = SessionMachine::new();
        let out = apply(
            &mut m,
            UiCommand::SetTitle {
                title: "z".repeat(150),
            },
            0,
        );
        assert_eq!(m.title().chars().count(), MAX_TITLE_CHARS);
        match out.events.as_slice() {
            [SessionEvent::TitleChanged { title }] => {
                assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
            }
            other => panic!("expected one TitleChanged, got {other:?}"),
        }
    }

    // -------- stop-hook policy reflects config --------

    #[test]
    fn stop_hooks_reflect_config_opt_outs() {
        let cfg = SessionConfig {
            ai_final_notes: false,
            smart_questions: true,
            smartq_recap: false, // recap off
            auto_summary: true,
            notes_model_off: false,
        };
        let mut m = SessionMachine::with_config(cfg);
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        let out = apply(&mut m, UiCommand::StopRecording, 1_000);
        let hooks = StopHooks {
            recluster: true,
            final_notes: false,
            question_recap: false, // smartq_recap=false ⇒ off
            auto_summary: true,
        };
        assert!(out.events.contains(&SessionEvent::StopHooks(hooks)));
    }

    #[test]
    fn question_recap_requires_both_flags() {
        // smart_questions off ⇒ recap off even if smartq_recap on.
        let cfg = SessionConfig {
            ai_final_notes: true,
            smart_questions: false,
            smartq_recap: true,
            auto_summary: false,
            notes_model_off: false,
        };
        let mut m = SessionMachine::with_config(cfg);
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        let out = apply(&mut m, UiCommand::StopRecording, 1_000);
        let Some(StopHooks { question_recap, .. }) = out.events.iter().find_map(|e| match e {
            SessionEvent::StopHooks(h) => Some(*h),
            _ => None,
        }) else {
            panic!("no StopHooks emitted")
        };
        assert!(!question_recap);
    }

    #[test]
    fn notes_model_off_is_authoritative_over_pass_flags() {
        // Transcript-only mode (Appendix A row 20): the empty notes slot forbids
        // BOTH model-driven passes even though their per-pass flags are on. The
        // model-free passes (recluster, auto_summary) are unaffected.
        let cfg = SessionConfig {
            ai_final_notes: true,
            smart_questions: true,
            smartq_recap: true,
            auto_summary: true,
            notes_model_off: true,
        };
        let mut m = SessionMachine::with_config(cfg);
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 0);
        let out = apply(&mut m, UiCommand::StopRecording, 1_000);
        let hooks = StopHooks {
            recluster: true,       // diarization — unrelated to the notes model
            final_notes: false,    // no notes model ⇒ no Qwen final notes
            question_recap: false, // no notes model ⇒ no Qwen question recap
            auto_summary: true,    // bridge/on-device summary — unrelated
        };
        assert!(
            out.events.contains(&SessionEvent::StopHooks(hooks)),
            "expected transcript-only StopHooks, got {:?}",
            out.events
        );
        assert!(out.effects.contains(&SideEffect::RunStopHooks(hooks)));
    }

    // -------- timer projection --------

    #[test]
    fn timer_text_elapsed_live_then_frozen() {
        let mut m = SessionMachine::new();
        // Before start: 00:00.
        assert_eq!(m.timer_text(0, None), "00:00");
        drive(
            &mut m,
            UiCommand::StartRecording { title: "x".into() },
            10_000,
        );
        // Live: now-start.
        assert_eq!(m.timer_text(40_000, None), "00:30");
        drive(&mut m, UiCommand::StopRecording, 130_000);
        // Frozen at the final 120s regardless of `now`.
        assert_eq!(m.timer_text(999_999, None), "02:00");
    }

    #[test]
    fn timer_text_clock_mode_uses_host_string_even_before_start() {
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::CycleTimestampMode, 0); // → clock
        assert_eq!(m.timer_text(0, Some("2:30 PM")), "2:30 PM");
        // No clock string ⇒ graceful elapsed fallback.
        assert_eq!(m.timer_text(0, None), "00:00");
    }

    #[test]
    fn format_stamp_modes() {
        let mut m = SessionMachine::new();
        drive(
            &mut m,
            UiCommand::StartRecording { title: "x".into() },
            1_000,
        );
        // elapsed: ts - start
        assert_eq!(m.format_stamp(31_000, 50_000, None), "00:30");

        drive(&mut m, UiCommand::CycleTimestampMode, 0); // → clock
        assert_eq!(m.format_stamp(0, 0, Some("3:00 PM")), "3:00 PM");

        drive(&mut m, UiCommand::CycleTimestampMode, 0); // → ago
        assert_eq!(m.format_stamp(0, 90_000, None), "1m ago");
    }

    // -------- full lifecycle / command-log replay --------

    #[test]
    fn full_lifecycle_start_stop_continue_stop_newmeeting() {
        let mut m = SessionMachine::new();
        // Idle → Recording (cold)
        let a = apply(
            &mut m,
            UiCommand::StartRecording {
                title: "Demo".into(),
            },
            0,
        );
        assert_eq!(a.effects, vec![SideEffect::LoadEngineAndCapture]);
        // Recording → Stopped
        drive(&mut m, UiCommand::StopRecording, 60_000);
        // Stopped → Recording (warm continue)
        let c = apply(&mut m, UiCommand::ResumeRecording, 70_000);
        assert_eq!(c.effects, vec![SideEffect::ResumeCaptureNoReload]);
        // Recording → Stopped again
        drive(&mut m, UiCommand::StopRecording, 90_000);
        // Stopped → Idle (new meeting)
        drive(&mut m, UiCommand::NewMeeting, 100_000);
        assert_eq!(m.state(), SessionState::Idle);
        assert!(!m.engine_loaded());
        assert_eq!(m.title(), DEFAULT_TITLE);
    }

    #[test]
    fn machine_is_serializable_for_command_log_replay() {
        // The machine round-trips through JSON, supporting command-log replay
        // (PRD "The UI boundary").
        let mut m = SessionMachine::new();
        drive(&mut m, UiCommand::StartRecording { title: "x".into() }, 5);
        let json = serde_json::to_string(&m).expect("serialize machine");
        let back: SessionMachine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
    }

    #[test]
    fn replaying_a_command_log_is_deterministic() {
        let log = [
            (
                UiCommand::SetTitle {
                    title: "Replay".into(),
                },
                0u64,
            ),
            (
                UiCommand::StartRecording {
                    title: String::new(),
                },
                1_000,
            ),
            (UiCommand::AddTabAudio, 2_000),
            (UiCommand::StopRecording, 8_000),
            (UiCommand::ResumeRecording, 9_000),
            (UiCommand::StopRecording, 12_000),
            (UiCommand::NewMeeting, 13_000),
        ];
        let run = || {
            let mut m = SessionMachine::new();
            let mut trace = Vec::new();
            for (cmd, now) in &log {
                trace.push(m.apply(cmd, *now));
            }
            (m, trace)
        };
        let (m1, t1) = run();
        let (m2, t2) = run();
        assert_eq!(m1, m2);
        assert_eq!(t1, t2);
        // Determinism: identical command logs produce identical final states and
        // identical per-step outcomes. The log ends on NewMeeting → Idle.
        assert_eq!(m1.state(), SessionState::Idle);
        assert!(!m1.engine_loaded());
    }

    #[test]
    fn empty_start_input_keeps_pending_set_title() {
        // SetTitle stashes "Replay"; an empty StartRecording input keeps it,
        // mirroring index.html reading the live (non-empty) input at start.
        let mut m = SessionMachine::new();
        drive(
            &mut m,
            UiCommand::SetTitle {
                title: "Replay".into(),
            },
            0,
        );
        drive(
            &mut m,
            UiCommand::StartRecording {
                title: String::new(),
            },
            1_000,
        );
        assert_eq!(m.title(), "Replay");
    }
}
