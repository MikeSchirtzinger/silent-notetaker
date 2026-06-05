//! Wasm-bindgen js-host engine surfaces (PRD Phase 5; Appendix A rows 10, 11 —
//! the heart of the strangler-fig).
//!
//! These are the thin browser boundaries over the four `silent_inference` engine
//! **policies** — the part the i2/i1 builders ported byte-for-byte from
//! `index.html`'s inline engine loops. Each wrapper holds the deterministic Rust
//! policy (chunking, VAD gate, hallucination filter, tail-dedup; Voxtral's
//! two-cap recycle; SenseVoice VAD segmentation + decode gate; the Dual
//! draft/refine interleaving) and exposes it over the same JSON boundary the
//! other silent-web surfaces use. The transformers.js worker / sherpa-onnx
//! harness are the **executors**: they run the model and report back; they hold
//! no policy (PRD R2).
//!
//! # The strangler-fig REPLACE
//!
//! The glue files (`whisper-engine.js`, `voxtral-engine.js`,
//! `sensevoice-engine.js`, `dual-engine.js`) REPLACE the in-`index.html` loops:
//!
//! - `whisper-engine.js` replaces the inlined `transcription-worker-src`
//!   buffer/splice + `hasSpeech`/`isHallucination`/`deduplicateText` loop and the
//!   `startMoonshine` main-thread chunker. The worker keeps only
//!   `transcriber(audio)`; everything else is [`WasmWhisperStream`] driving it.
//! - `voxtral-engine.js` replaces the `_runVoxtralTranscription` outer-while
//!   recycle + `flushDecodedText` in-place partial machine. The host keeps only
//!   `model.generate(...)` + the mel/ring plumbing; the two caps, the recycle
//!   seam, and the partial/sentence slicing are [`WasmVoxtralRecycle`] (row 10).
//! - `sensevoice-engine.js` replaces the `SenseVoiceEngine.processAudio`
//!   buffer/window-drain + `> 0.3 s` decode gate. The sherpa harness keeps only
//!   `vad.acceptWaveform` + `recognizer.decode`; the windowing + gate are
//!   [`WasmSenseVoice`] (row 11).
//! - `dual-engine.js` interleaves both legs through [`WasmDual`], whose
//!   coordinator owns the draft/refine supersede-on-refine list policy (row 11).
//!
//! # Event shape
//!
//! Transcript outputs cross as `silent_core::EngineEvent` JSON
//! (`{ "tag": "...", "payload": ... }`, snake_case tags) — the SAME convention as
//! [`crate::nemotron`], so the UI's render path is engine-agnostic. The policies
//! emit `silent_inference::TextEvent` (Partial/Final, range-free); this surface
//! is where the [`TimeRange`] is attached (the policy has the span from the
//! `HostCommand` it issued; the host echoes it back). `HostCommand`s and the
//! Dual `ListEdit`s cross as their own serde-JSON (the policies' wire types).
//!
//! # bigint coercion at the boundary
//!
//! Sample positions and ms spans are `u64` in the policy. JS numbers are `f64`,
//! exact for integers below 2^53 — a session's sample count (16 kHz × hours) and
//! ms span stay far under that, so the glue passes plain JS numbers and this
//! surface coerces `f64` → `u64` at the boundary (clamped non-negative). The
//! `HostCommand` JSON the glue round-trips carries `u64` fields as JSON numbers
//! the same way.
//!
//! # wasm32-only
//!
//! Compiled only for `wasm32-unknown-unknown`; the native workspace build gates
//! this module out (see `lib.rs`), so `cargo check --workspace` stays free of
//! wasm-bindgen. The policies themselves are native-tested in `silent-inference`.

use wasm_bindgen::prelude::*;

use silent_core::events::EngineEvent;
use silent_core::ids::TimeRange;
use silent_inference::TextEvent;
use silent_inference::dual::DualCoordinator;
use silent_inference::sensevoice::{SenseVoiceConfig, SenseVoicePolicy};
use silent_inference::voxtral_recycle::{RecycleConfig, VoxtralRecyclePolicy};
use silent_inference::whisper_stream::{WhisperStreamConfig, WhisperStreamPolicy};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `JsError` from any `Display` error (a loud failure, never a silent
/// drop — PRD "loud failures").
fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Serialize a value to a JSON string `JsValue` the glue `JSON.parse`s — the same
/// serde-JSON convention as [`crate::nemotron`] / [`crate::selection`].
fn to_js_value<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(v).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

/// Coerce a JS number (`f64`) carrying a non-negative integer into the `u64` the
/// policy uses for sample positions / ms spans. Clamps negatives to 0 (a negative
/// position is never legitimate; clamping is loud-by-absurdity rather than a
/// silent wrap).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample positions / ms spans are non-negative integers well under \
              2^53 (the JS exact-integer range); the f64 boundary is the JS number \
              type and the clamp guards the sign"
)]
fn u64_at_boundary(n: f64) -> u64 {
    n.max(0.0) as u64
}

/// Map a `silent_inference::TextEvent` (range-free) to a `silent_core::EngineEvent`
/// stamped with the given [`TimeRange`]. The policy returns the text decision; the
/// span comes from the `HostCommand` the host executed (echoed back by the glue).
fn text_event_to_engine(ev: &TextEvent, range: TimeRange) -> EngineEvent {
    match ev {
        TextEvent::Partial(text) => EngineEvent::Partial {
            text: text.clone(),
            range,
        },
        TextEvent::Final(text) => EngineEvent::Final {
            text: text.clone(),
            range,
        },
        // TextEvent is #[non_exhaustive]; an additive kind is not produced by the
        // current policies. Treat any future kind as a final segment so the UI
        // still surfaces it rather than dropping text silently.
        _ => EngineEvent::Final {
            text: String::new(),
            range,
        },
    }
}

// ---------------------------------------------------------------------------
// WasmWhisperStream — Whisper family + Moonshine solo (Appendix A rows 7, 11)
// ---------------------------------------------------------------------------

/// Browser-facing Whisper-family / Moonshine streaming policy.
///
/// Drives the existing transformers.js transcription worker: the glue pushes
/// captured 16 kHz samples ([`push_samples`](WasmWhisperStream::push_samples)),
/// gets back the `HostCommand::Transcribe` chunks to post to the worker, and
/// feeds each decoded chunk's text back ([`on_decoded`](WasmWhisperStream::on_decoded))
/// to get the post-filter [`EngineEvent`]s. The policy owns chunk boundaries, the
/// VAD gate, the hallucination filter, and tail-dedup; the worker only runs
/// `transcriber(audio)`.
#[wasm_bindgen]
pub struct WasmWhisperStream {
    policy: WhisperStreamPolicy,
}

#[wasm_bindgen]
impl WasmWhisperStream {
    /// Solo Whisper / Moonshine: 5 s chunks, shipping VAD threshold
    /// (`WhisperStreamConfig::WHISPER_SOLO`). The default for the solo engines.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> WasmWhisperStream {
        console_error_panic_hook::set_once();
        WasmWhisperStream {
            policy: WhisperStreamPolicy::new(WhisperStreamConfig::WHISPER_SOLO),
        }
    }

    /// Moonshine **in Dual mode**: 3 s chunks for faster draft feedback
    /// (`WhisperStreamConfig::MOONSHINE_DUAL`). Used by [`WasmDual`]'s Moonshine
    /// leg; exposed standalone so the Dual glue can drive the same policy code.
    #[wasm_bindgen(js_name = moonshineDual)]
    #[must_use]
    pub fn moonshine_dual() -> WasmWhisperStream {
        console_error_panic_hook::set_once();
        WasmWhisperStream {
            policy: WhisperStreamPolicy::new(WhisperStreamConfig::MOONSHINE_DUAL),
        }
    }

    /// Push captured 16 kHz mono samples; returns the `HostCommand[]` JSON the glue
    /// posts to the worker (`{ "cmd": "transcribe", "chunk", "start_ms", "end_ms" }`
    /// per ready, speech-bearing chunk). Silent chunks are dropped by the VAD gate
    /// and produce no command (identical observable behavior to the JS `continue`).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = pushSamples)]
    pub fn push_samples(&mut self, samples: &[f32]) -> Result<JsValue, JsError> {
        let (commands, _skips) = self.policy.push_samples(samples);
        to_js_value(&commands)
    }

    /// Feed the worker's decoded text for one chunk back through the post-decode
    /// filters (length guard, hallucination filter, tail-dedup) and return the
    /// resulting `EngineEvent[]` JSON — a single `Final` (range-stamped with the
    /// chunk's span) or an empty array when the text was dropped.
    ///
    /// `start_ms`/`end_ms` are the span of the `HostCommand::Transcribe` the worker
    /// executed (the glue echoes them back from the command it posted), so the
    /// emitted `Final` carries the right [`TimeRange`].
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onDecoded)]
    pub fn on_decoded(
        &mut self,
        decoded: &str,
        start_ms: f64,
        end_ms: f64,
    ) -> Result<JsValue, JsError> {
        let range = TimeRange::new(u64_at_boundary(start_ms), u64_at_boundary(end_ms));
        let events: Vec<EngineEvent> = self
            .policy
            .on_decoded(decoded)
            .iter()
            .map(|ev| text_event_to_engine(ev, range))
            .collect();
        to_js_value(&events)
    }

    /// Request stop (the worker `terminate`). The next
    /// [`drain_finalize`](WasmWhisperStream::drain_finalize) emits the single
    /// `HostCommand::Finalize`.
    #[wasm_bindgen(js_name = requestStop)]
    pub fn request_stop(&mut self) {
        self.policy.request_stop();
    }

    /// Emit the one-shot `HostCommand::Finalize` after stop (the worker teardown).
    /// Returns the command JSON, or JSON `null` before stop / after finalize.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = drainFinalize)]
    pub fn drain_finalize(&mut self) -> Result<JsValue, JsError> {
        match self.policy.drain_finalize() {
            Some(cmd) => to_js_value(&cmd),
            None => Ok(JsValue::null()),
        }
    }

    /// Clear all streaming state for a fresh utterance/session (keeps the config).
    pub fn reset(&mut self) {
        self.policy.reset();
    }
}

impl Default for WasmWhisperStream {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WasmVoxtralRecycle — Voxtral two-cap recycle (Appendix A row 10)
// ---------------------------------------------------------------------------

/// Browser-facing Voxtral two-cap context-recycle policy.
///
/// Drives the transformers.js Voxtral worker: the glue reports the host's progress
/// (context started / tokens / audio advanced / decoded text) and calls
/// [`poll`](WasmVoxtralRecycle::poll) to get the next `HostCommand`
/// (`StartContext` / `Recycle` / `Finalize`). The policy owns the token cap, the
/// audio/time cap, the recycle seam, and the in-place partial/sentence slicing;
/// the worker only runs `model.generate(...)` and streams tokens back.
///
/// Built with `RecycleConfig::VOXTRAL_SHIPPING` by default (the 320-token / 45 s
/// caps the 2026-05-29 RAM fix landed); a smaller test config is available for a
/// witnessed forced-recycle run.
#[wasm_bindgen]
pub struct WasmVoxtralRecycle {
    policy: VoxtralRecyclePolicy,
}

#[wasm_bindgen]
impl WasmVoxtralRecycle {
    /// Shipping Voxtral caps (320-token / 45 s) — `RecycleConfig::VOXTRAL_SHIPPING`.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> WasmVoxtralRecycle {
        console_error_panic_hook::set_once();
        WasmVoxtralRecycle {
            policy: VoxtralRecyclePolicy::new(RecycleConfig::VOXTRAL_SHIPPING),
        }
    }

    /// A TEST config with small caps so a forced-recycle event can be witnessed in
    /// a short run, then the shipping caps restored by recreating the engine. The
    /// caps are passed explicitly (token count, audio-cap samples) so the glue can
    /// force, e.g., a recycle after a few seconds of audio.
    ///
    /// Do NOT use in production paths — the shipping caps are the RAM fix. This
    /// exists solely for the witnessed-recycle acceptance run (PRD I5).
    #[wasm_bindgen(js_name = withTestCaps)]
    #[must_use]
    pub fn with_test_caps(max_new_tokens: u32, max_ctx_samples: f64) -> WasmVoxtralRecycle {
        console_error_panic_hook::set_once();
        WasmVoxtralRecycle {
            policy: VoxtralRecyclePolicy::new(RecycleConfig {
                max_new_tokens,
                max_ctx_samples: u64_at_boundary(max_ctx_samples),
            }),
        }
    }

    /// Drive the loop one step: returns the next `HostCommand` JSON the host must
    /// execute (`StartContext` / `Recycle` / `Finalize`), or JSON `null` while the
    /// active context keeps streaming.
    ///
    /// `ring_write_abs` is the current ring write position (advances only);
    /// `prompt_tokens` is the prompt size the host measured for the next context.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn poll(&mut self, ring_write_abs: f64, prompt_tokens: u32) -> Result<JsValue, JsError> {
        match self
            .policy
            .poll(u64_at_boundary(ring_write_abs), prompt_tokens)
        {
            Some(cmd) => to_js_value(&cmd),
            None => Ok(JsValue::null()),
        }
    }

    /// Host event: the `generate` context started, with the host's measured prompt
    /// token count.
    #[wasm_bindgen(js_name = onContextStarted)]
    pub fn on_context_started(&mut self, prompt_tokens: u32) {
        self.policy.on_context_started(prompt_tokens);
    }

    /// Host event: the worker emitted `n` new tokens (a `streamer.put`).
    #[wasm_bindgen(js_name = onTokens)]
    pub fn on_tokens(&mut self, n: u32) {
        self.policy.on_tokens(n);
    }

    /// Host event: the active context has consumed audio up to `consumed_abs` (ring
    /// position of the last fed sample), anchored at `anchor_abs` (echoed from the
    /// `StartContext` the host executed).
    #[wasm_bindgen(js_name = onAudioAdvanced)]
    pub fn on_audio_advanced(&mut self, anchor_abs: f64, consumed_abs: f64) {
        self.policy
            .on_audio_advanced(u64_at_boundary(anchor_abs), u64_at_boundary(consumed_abs));
    }

    /// Feed a cumulative decoded-text snapshot (`tokenizer.decode(tokenCache)`) into
    /// the in-place partial-text machine; returns the `EngineEvent[]` JSON — a
    /// `Partial` (the live in-place text) and, when a sentence completes, a `Final`.
    ///
    /// `start_ms`/`end_ms` stamp the events with the active context's audio span
    /// (the glue derives it from the ring positions it reports). Voxtral's in-place
    /// partial semantics are preserved exactly (row 10): the same `Partial` text the
    /// JS overwrote the live element with, the same `Final` on each sentence.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onDecodedText)]
    pub fn on_decoded_text(
        &mut self,
        decoded: &str,
        start_ms: f64,
        end_ms: f64,
    ) -> Result<JsValue, JsError> {
        let range = TimeRange::new(u64_at_boundary(start_ms), u64_at_boundary(end_ms));
        let events: Vec<EngineEvent> = self
            .policy
            .on_decoded_text(decoded)
            .iter()
            .map(|ev| text_event_to_engine(ev, range))
            .collect();
        to_js_value(&events)
    }

    /// End-of-context text flush (`streamer.end()`): emit any trailing sentence
    /// buffer as a `Final` and reset the per-context text state. Returns the
    /// `EngineEvent[]` JSON.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onContextEndText)]
    pub fn on_context_end_text(&mut self, start_ms: f64, end_ms: f64) -> Result<JsValue, JsError> {
        let range = TimeRange::new(u64_at_boundary(start_ms), u64_at_boundary(end_ms));
        let events: Vec<EngineEvent> = self
            .policy
            .on_context_end_text()
            .iter()
            .map(|ev| text_event_to_engine(ev, range))
            .collect();
        to_js_value(&events)
    }

    /// Request stop (`isStop()` latches true). The next `poll` emits the single
    /// `HostCommand::Finalize`.
    #[wasm_bindgen(js_name = requestStop)]
    pub fn request_stop(&mut self) {
        self.policy.request_stop();
    }

    /// Whether a `generate` context is currently active (drives the glue's host
    /// progress reporting, mirroring the JS `Diag` `is_running` guard).
    #[wasm_bindgen(js_name = isRunning)]
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.policy.is_running()
    }

    /// The aggregate session counters (contexts/recycles/token totals) — the `Diag`
    /// trail the PerfMonitor / I5 flat-memory acceptance reads. Returns the
    /// `SessionStats` JSON.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = sessionStats)]
    pub fn session_stats(&self) -> Result<JsValue, JsError> {
        // SessionStats derives Serialize? It does not — build a small JSON object.
        let s = self.policy.session_stats();
        let v = serde_json::json!({
            "contexts_started": s.contexts_started,
            "recycles": s.recycles,
            "tokens_total": s.tokens_total,
            "token_cap_recycles": s.token_cap_recycles,
            "audio_cap_recycles": s.audio_cap_recycles,
        });
        to_js_value(&v)
    }
}

impl Default for WasmVoxtralRecycle {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WasmSenseVoice — SenseVoice solo VAD segmentation (Appendix A row 11)
// ---------------------------------------------------------------------------

/// Browser-facing SenseVoice solo VAD-segmentation policy.
///
/// Drives the sherpa-onnx (js-sherpa) harness: the glue pushes the captured sample
/// COUNT ([`push_samples`](WasmSenseVoice::push_samples)) to get the
/// `HostCommand::FeedVadWindow` commands (which window offsets to feed the VAD),
/// and reports each VAD-detected segment's length
/// ([`on_vad_segment`](WasmSenseVoice::on_vad_segment)) to get the `> 0.3 s` decode
/// gate decision and, when it passes, a `HostCommand::DecodeSegment`. The host runs
/// the VAD + ASR; the policy owns the buffering, windowing, and the decode gate.
///
/// The host owns the actual samples (sherpa's `CircularBuffer`); the policy works
/// purely from counts + reported segment lengths — so it is browser-free and
/// native-tested.
#[wasm_bindgen]
pub struct WasmSenseVoice {
    policy: SenseVoicePolicy,
}

#[wasm_bindgen]
impl WasmSenseVoice {
    /// Shipping SenseVoice config (`SenseVoiceConfig::SHIPPING`): the 512-sample VAD
    /// window, the 30 s max-speech window, the `> 0.3 s` decode gate.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> WasmSenseVoice {
        console_error_panic_hook::set_once();
        WasmSenseVoice {
            policy: SenseVoicePolicy::new(SenseVoiceConfig::SHIPPING),
        }
    }

    /// Push the count of newly captured 16 kHz samples; returns the
    /// `HostCommand[]` JSON — one `FeedVadWindow` (with its absolute sample offset)
    /// per whole 512-sample window now drainable. Mirrors the JS
    /// `while (buffer.size() > windowSize)` drain exactly (strict `>`, holds one
    /// window back).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = pushSamples)]
    pub fn push_samples(&mut self, sample_count: u32) -> Result<JsValue, JsError> {
        let cmds = self.policy.push_samples(sample_count as usize);
        to_js_value(&cmds)
    }

    /// Report a VAD-detected segment (the host's `vad.front()`); returns a JSON
    /// object `{ decision, command }` where `decision` is `"decode"` / `"too_short"`
    /// and `command` is the `HostCommand::DecodeSegment` JSON (or `null` when the
    /// segment failed the `> 0.3 s` gate). The boundary case (exactly 0.3 s →
    /// dropped) matches the JS float compare byte-for-byte.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onVadSegment)]
    pub fn on_vad_segment(
        &mut self,
        offset_samples: f64,
        length_samples: f64,
    ) -> Result<JsValue, JsError> {
        let (decision, command) = self.policy.on_vad_segment(
            u64_at_boundary(offset_samples),
            u64_at_boundary(length_samples),
        );
        let v = serde_json::json!({
            "decision": decision,
            "command": command,
        });
        to_js_value(&v)
    }

    /// Request stop (the host `destroy()`). The next
    /// [`drain_finalize`](WasmSenseVoice::drain_finalize) emits the single
    /// `HostCommand::Finalize`.
    #[wasm_bindgen(js_name = requestStop)]
    pub fn request_stop(&mut self) {
        self.policy.request_stop();
    }

    /// Emit the one-shot `HostCommand::Finalize` after stop (the recognizer/VAD
    /// teardown). Returns the command JSON, or JSON `null` before stop / after
    /// finalize.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = drainFinalize)]
    pub fn drain_finalize(&mut self) -> Result<JsValue, JsError> {
        match self.policy.drain_finalize() {
            Some(cmd) => to_js_value(&cmd),
            None => Ok(JsValue::null()),
        }
    }

    /// Clear all streaming state for a fresh session (keeps the config).
    pub fn reset(&mut self) {
        self.policy.reset();
    }
}

impl Default for WasmSenseVoice {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WasmDual — Dual-mode draft/refine coordination (Appendix A row 11)
// ---------------------------------------------------------------------------

/// Browser-facing Dual-mode draft/refine coordinator.
///
/// Wraps the [`DualCoordinator`] — the authoritative transcript-item list policy.
/// The Moonshine leg (a [`WasmWhisperStream`] at the 3 s Dual cadence) and the
/// SenseVoice leg (a [`WasmSenseVoice`]) run concurrently on the same audio; this
/// coordinator interleaves them: a Moonshine final appends a **draft**, a
/// SenseVoice refined final supersedes the older drafts (keeping at most one as a
/// preview) and appends the **refined** item.
///
/// Each method returns the `ListEdit[]` JSON the glue applies to the DOM
/// (`{ "op": "append", "item": { text, draft } }` / `{ "op": "remove", "index" }`),
/// the same supersede-on-refine rule the inline `handlePartial`/`handleFinal` Dual
/// branches implemented.
#[wasm_bindgen]
pub struct WasmDual {
    coordinator: DualCoordinator,
}

#[wasm_bindgen]
impl WasmDual {
    /// A fresh Dual coordinator with an empty transcript list.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> WasmDual {
        console_error_panic_hook::set_once();
        WasmDual {
            coordinator: DualCoordinator::new(),
        }
    }

    /// Moonshine produced a final → append a draft item. Returns the `ListEdit[]`
    /// JSON (a single `Append`, or empty for empty/whitespace text).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onMoonshineFinal)]
    pub fn on_moonshine_final(&mut self, text: &str) -> Result<JsValue, JsError> {
        to_js_value(&self.coordinator.on_moonshine_final(text))
    }

    /// SenseVoice produced a refined final → supersede the older drafts and append
    /// the refined item. Returns the `ListEdit[]` JSON (the draft `Remove`s in
    /// ascending pre-edit index, then the refined `Append`; empty for empty text).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    #[wasm_bindgen(js_name = onSenseVoiceFinal)]
    pub fn on_sensevoice_final(&mut self, text: &str) -> Result<JsValue, JsError> {
        to_js_value(&self.coordinator.on_sensevoice_final(text))
    }

    /// The authoritative transcript list snapshot (`TranscriptItem[]` JSON) — the
    /// unambiguous source of truth the glue can re-mirror (the `ListEdit`s are the
    /// incremental path; this is the whole-list fallback).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn items(&self) -> Result<JsValue, JsError> {
        to_js_value(&self.coordinator.items())
    }

    /// Reset to an empty list (new meeting / fresh session).
    pub fn reset(&mut self) {
        self.coordinator.reset();
    }
}

impl Default for WasmDual {
    fn default() -> Self {
        Self::new()
    }
}
