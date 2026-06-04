//! Voxtral's token/audio two-cap context recycle, as a Rust policy module.
//!
//! This is the hardest-won bug fix in the app made into tested policy (PRD R2;
//! Appendix A row 10: *"Voxtral streaming with in-place partial text and two-cap
//! context recycle"*). The JS original lives in `index.html`
//! `_runVoxtralTranscription` (the RAM-FIX comment dated 2026-05-29). It is
//! ported here byte-for-byte in behavior, then exercised by deterministic tests
//! that do not require a browser.
//!
//! # The bug it fixes
//!
//! Each transformers.js `model.generate(...)` call is **one streaming context**.
//! Its KV cache (`past_key_values`) grows with every emitted token and is owned
//! *internally* by `generate` — it cannot be freed mid-stream without breaking
//! the streaming contract. Left unbounded (`max_new_tokens: 4096`) a single
//! context creeps toward ~2 GB of KV + arena and the tab locks up after ~5 min.
//!
//! # The fix: two independent caps, one recycle
//!
//! 1. **Token cap** ([`RecycleConfig::max_new_tokens`], JS `max_new_tokens: 320`).
//!    At ~0.52 MB/token (measured, M1 Metal, real Voxtral 4B) 320 tokens ≈ 166 MB
//!    peak; recycling drops it back down.
//! 2. **Audio/time cap** ([`RecycleConfig::max_ctx_samples`], JS
//!    `MAX_CTX_SAMPLES = 16000 * 45` = 45 s). Catches *slow-token* contexts that
//!    consume lots of audio without emitting many tokens (quiet periods,
//!    non-speech) — they would otherwise creep toward the token cap without ever
//!    tripping it. It works **independently** of the token cap.
//!
//! Both caps are handled by the *same* recycle: the current context ends, and a
//! fresh context is anchored at the **current ring write position**, so no audio
//! is skipped and no evicted audio is re-read — *"transcription is continuous
//! across seams"* (the JS outer-`while` comment).
//!
//! # Policy vs execution (the `JsHostEngine` boundary)
//!
//! This module is **pure Rust law**. It owns the decisions — *when* to start a
//! context, *what* audio span it covers, *when* to recycle and *why* — and emits
//! typed [`HostCommand`]s. The transformers.js worker (later wiring, in
//! `silent-web`) is the executor: it runs `generate`/decode and returns token
//! deltas. It carries **no** thresholds, no recycle logic, no chunk-size math
//! (PRD R2; b2 spike `docs/research/spike-jshost.md`). The split lets the policy
//! be unit-tested with simulated token/audio streams — which is the whole point.
//!
//! No `unwrap`/`expect` on any path; the module has no fallible operations on the
//! hot path (PRD "Rust engineering bar").

use serde::{Deserialize, Serialize};

use crate::TextEvent;

/// 16 kHz mono is the rate every engine expects ([`silent_core::events::AsrCapabilities`]).
/// Ring positions and audio caps are counted in samples at this rate.
const SAMPLE_RATE_HZ: u64 = 16_000;

/// Configuration for the two caps, mirroring the JS constants.
///
/// In the running app these arrive as registry config (PRD R4 / Task I3); here
/// they are explicit so the policy can be pinned to the exact shipping values
/// and so tests can shrink them to exercise each cap deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecycleConfig {
    /// **Token cap.** Max tokens a single `generate` context may emit before it
    /// returns and the policy recycles. JS `max_new_tokens: 320`.
    pub max_new_tokens: u32,
    /// **Audio/time cap**, in samples at 16 kHz. If a context consumes more than
    /// this many ring samples (measured from its anchor) the policy ends it and
    /// recycles, regardless of token count. JS `MAX_CTX_SAMPLES = 16000 * 45`.
    pub max_ctx_samples: u64,
}

impl RecycleConfig {
    /// The exact shipping Voxtral values: 320-token cap, 45 s audio cap.
    ///
    /// These are the constants the 2026-05-29 RAM fix landed in `index.html`.
    pub const VOXTRAL_SHIPPING: RecycleConfig = RecycleConfig {
        max_new_tokens: 320,
        max_ctx_samples: SAMPLE_RATE_HZ * 45,
    };

    /// The audio cap expressed in seconds (for diagnostics / display).
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "audio caps are small sample counts (the shipping cap is 720_000; \
                  any realistic cap is well under 2^53), so the u64→f64 conversion \
                  is exact. This is a display/diagnostics helper, never on the hot path."
    )]
    pub fn max_ctx_seconds(&self) -> f64 {
        self.max_ctx_samples as f64 / SAMPLE_RATE_HZ as f64
    }
}

impl Default for RecycleConfig {
    fn default() -> Self {
        Self::VOXTRAL_SHIPPING
    }
}

/// Why a context was (or must be) recycled — the two cap decision points, plus
/// stop and the upstream "ring moved under us" end.
///
/// This is the typed form of what the JS code split across the audio-cap `break`
/// inside the mel generator, the `max_new_tokens` ceiling inside `generate`, and
/// the `isStop()` guards. Making it an enum is what lets a test assert *which*
/// cap fired, and what lets `Diag` classify recycles (`recycleCount`).
///
/// `#[non_exhaustive]` so a future cap (e.g. a wall-clock cap) is an additive
/// change, matching the boundary-event convention in `silent-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RecycleReason {
    /// The token cap was reached: the context emitted [`RecycleConfig::max_new_tokens`]
    /// tokens. JS: `generate` returns on `max_new_tokens`.
    TokenCap,
    /// The audio/time cap was reached: the context consumed more than
    /// [`RecycleConfig::max_ctx_samples`] of ring audio. JS:
    /// `if (startIdx - baseAbs > MAX_CTX_SAMPLES) break;` in the mel generator.
    AudioCap,
    /// A stop was requested (or a newer run superseded this loop). JS: `isStop()`.
    /// No fresh context is started after this reason.
    Stop,
}

/// A typed command the policy emits for the JS host (transformers.js worker) to
/// execute. Mirrors the b2 spike's `HostCommand` shape and the PRD's versioned
/// command protocol; `#[serde(tag = "cmd")]` gives the JS side a discriminated
/// union. The host **executes**, it does not decide.
///
/// `#[non_exhaustive]` so adding a command (e.g. a warm-up ping) does not break
/// the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostCommand {
    /// Start a fresh `generate` context. The host calls `model.generate(...)`
    /// with `max_new_tokens` and streams mel features forward from `anchor_abs`
    /// (the ring write position at start time). JS: the top of the outer `while`,
    /// `const baseAbs = ring.writeAbs;` plus the `max_new_tokens` arg.
    StartContext {
        /// Monotonic context index (JS `Diag.loopIter`). Lets the host and tests
        /// correlate events with the context that produced them.
        context: u32,
        /// Ring write position this context is anchored at — it reads forward
        /// from here and never re-reads evicted samples. JS `baseAbs`.
        anchor_abs: u64,
        /// Token ceiling for this context (`generate`'s `max_new_tokens`). JS 320.
        max_new_tokens: u32,
    },

    /// Recycle: the active context has ended; the policy will issue a fresh
    /// [`HostCommand::StartContext`] on the next call unless stopped. JS: the
    /// outer-`while` recycle (`Diag.onRecycle()`), driven by either cap.
    ///
    /// Emitted with the [`RecycleReason`] so the host/diagnostics can classify it.
    /// Carries the [`ContextStats`] for the context that just ended so bounded
    /// growth is observable per seam (PRD R9 / Appendix A row 34).
    Recycle {
        /// The context index that ended. JS `Diag.loopIter` at recycle time.
        context: u32,
        /// Which cap (or stop) ended the context.
        reason: RecycleReason,
        /// Final stats for the ended context.
        stats: ContextStats,
    },

    /// End of stream: the host should tear down the worker. Emitted once, after
    /// the loop exits on stop. JS: loop falls out of the `while` and the promise
    /// resolves.
    Finalize {
        /// Stats for the final context, if one was active when stop arrived.
        stats: Option<ContextStats>,
    },
}

/// Per-context counters mirroring the `Diag` trail (`index.html` ~1872-1945).
///
/// These are the **proof of bounded growth**: a healthy session shows
/// `tokens_emitted` capped at [`RecycleConfig::max_new_tokens`] and
/// `audio_samples` capped near [`RecycleConfig::max_ctx_samples`] *per context*,
/// resetting on every recycle — the bounded sawtooth, not unbounded growth (the
/// runaway-RAM symptom the fix eliminated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContextStats {
    /// Tokens this context has emitted (JS `Diag.ctxLen`, reset per context).
    pub tokens_emitted: u32,
    /// Ring samples this context has consumed since its anchor (JS
    /// `startIdx - baseAbs`).
    pub audio_samples: u64,
    /// Prompt (`input_ids`) token count this context started with (JS
    /// `Diag.inputTokens`). The recycle anchor keeps this bounded; it is recorded
    /// so a regression that lets the prompt grow is visible.
    pub prompt_tokens: u32,
}

/// Aggregate session counters, mirroring the `Diag` global trail. Used by the
/// 10-minute simulation test to assert *session-level* bounded growth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStats {
    /// Number of fresh `generate` contexts started (JS `Diag.loopIter`).
    pub contexts_started: u32,
    /// Number of recycle events (cap-driven; excludes the final stop). JS
    /// `Diag.recycleCount`.
    pub recycles: u32,
    /// Total tokens emitted across the whole session (JS `Diag.genStepsTotal`,
    /// in token units).
    pub tokens_total: u64,
    /// Recycles attributed to the token cap.
    pub token_cap_recycles: u32,
    /// Recycles attributed to the audio/time cap.
    pub audio_cap_recycles: u32,
}

/// The lifecycle state of the recycle loop. Models the JS outer `while` plus the
/// `isStop()` short-circuit explicitly so illegal transitions are unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopState {
    /// No context active; the next [`VoxtralRecyclePolicy::poll`] starts one.
    Idle,
    /// A context is active and accumulating tokens/audio.
    Running,
    /// Stop requested; the loop is finished. JS `isStop()` is latched true.
    Stopped,
}

/// The Voxtral two-cap context-recycle policy.
///
/// Drive it with the typed events the host reports — [`on_context_started`],
/// [`on_tokens`], [`on_audio_advanced`] — and call [`poll`] to get the next
/// [`HostCommand`]. The policy decides *everything*; the host only executes the
/// commands and reports back what happened.
///
/// [`on_context_started`]: VoxtralRecyclePolicy::on_context_started
/// [`on_tokens`]: VoxtralRecyclePolicy::on_tokens
/// [`on_audio_advanced`]: VoxtralRecyclePolicy::on_audio_advanced
/// [`poll`]: VoxtralRecyclePolicy::poll
#[derive(Debug, Clone)]
pub struct VoxtralRecyclePolicy {
    config: RecycleConfig,
    state: LoopState,
    /// Monotonic context index; increments per fresh context (JS `loopIter`).
    next_context: u32,
    /// Stats for the context currently active (if [`LoopState::Running`]).
    current: ContextStats,
    /// The current context's index (the one [`StartContext`] was issued for).
    ///
    /// [`StartContext`]: HostCommand::StartContext
    current_index: u32,
    /// Aggregate session counters.
    session: SessionStats,
    /// In-place partial-text accumulation (the `tokenCache`/`printLen`/
    /// `sentenceBuffer` port). One per context; reset on recycle.
    text: PartialText,
}

impl VoxtralRecyclePolicy {
    /// Build a policy with the given caps. Use [`RecycleConfig::VOXTRAL_SHIPPING`]
    /// for the exact production behavior.
    #[must_use]
    pub fn new(config: RecycleConfig) -> Self {
        Self {
            config,
            state: LoopState::Idle,
            next_context: 0,
            current: ContextStats::default(),
            current_index: 0,
            session: SessionStats::default(),
            text: PartialText::new(),
        }
    }

    /// The caps in force.
    #[must_use]
    pub fn config(&self) -> RecycleConfig {
        self.config
    }

    /// Aggregate session counters (JS `Diag` global trail).
    #[must_use]
    pub fn session_stats(&self) -> SessionStats {
        self.session
    }

    /// Stats for the active context (zeroed when idle/stopped).
    #[must_use]
    pub fn current_stats(&self) -> ContextStats {
        self.current
    }

    /// Whether a context is currently active.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.state, LoopState::Running)
    }

    /// Drive the loop one step. Returns the next [`HostCommand`] the host must
    /// execute, or `None` when the loop has stopped and finalized.
    ///
    /// This is the outer-`while` body, made pull-based so it is deterministic and
    /// testable:
    ///
    /// - **Idle** → start a fresh context anchored at `ring_write_abs` (JS
    ///   `baseAbs = ring.writeAbs`), with `prompt_tokens` as the context's prompt
    ///   size, and emit [`HostCommand::StartContext`].
    /// - **Running** → if a cap is tripped, end the context and emit
    ///   [`HostCommand::Recycle`] (the loop returns to Idle; the *next* `poll`
    ///   starts the seam context). Otherwise `None` — keep streaming.
    /// - **Stopped** → emit a single [`HostCommand::Finalize`], then `None`.
    ///
    /// `ring_write_abs` is the current ring write position (it only ever
    /// advances) and `prompt_tokens` is the prompt size the host measured for the
    /// next context's first chunk (JS `Diag.onLoopIter(inTok)`).
    pub fn poll(&mut self, ring_write_abs: u64, prompt_tokens: u32) -> Option<HostCommand> {
        match self.state {
            LoopState::Idle => Some(self.start_context(ring_write_abs, prompt_tokens)),
            LoopState::Running => self.maybe_recycle(),
            LoopState::Stopped => self.take_finalize(),
        }
    }

    /// Start a fresh context anchored at the current ring position. The seam
    /// guarantee: the new context reads forward from `anchor_abs` and never
    /// re-reads evicted audio.
    fn start_context(&mut self, anchor_abs: u64, prompt_tokens: u32) -> HostCommand {
        let context = self.next_context;
        self.next_context += 1;
        self.current = ContextStats {
            tokens_emitted: 0,
            audio_samples: 0,
            prompt_tokens,
        };
        self.current_index = context;
        self.text.reset(); // per-context text state (JS: fresh tokenCache/printLen/buffer)
        self.session.contexts_started += 1;
        self.state = LoopState::Running;
        HostCommand::StartContext {
            context,
            anchor_abs,
            max_new_tokens: self.config.max_new_tokens,
        }
    }

    /// Check the two caps; if either is tripped, end the context and return a
    /// [`HostCommand::Recycle`]. The loop drops to Idle so the next `poll` opens
    /// the seam context. Returns `None` while the context may keep running.
    ///
    /// **Cap precedence** matches the JS exactly: the audio/time cap is evaluated
    /// inside the mel generator *before* the next chunk is fed, so a context that
    /// reaches the audio cap recycles for [`RecycleReason::AudioCap`] even if it
    /// also happens to be at the token cap. The token cap is the secondary hard
    /// ceiling (the JS comment: *"Audio/time cap … is the primary guard for
    /// slow-token contexts; this is the secondary hard token ceiling."*).
    fn maybe_recycle(&mut self) -> Option<HostCommand> {
        let audio_capped = self.current.audio_samples > self.config.max_ctx_samples;
        let token_capped = self.current.tokens_emitted >= self.config.max_new_tokens;

        if !audio_capped && !token_capped {
            return None;
        }
        let reason = if audio_capped {
            RecycleReason::AudioCap
        } else {
            RecycleReason::TokenCap
        };
        Some(self.recycle(reason))
    }

    /// End the active context and record the recycle. Drops to Idle.
    fn recycle(&mut self, reason: RecycleReason) -> HostCommand {
        let stats = self.current;
        self.session.recycles += 1;
        match reason {
            RecycleReason::TokenCap => self.session.token_cap_recycles += 1,
            RecycleReason::AudioCap => self.session.audio_cap_recycles += 1,
            RecycleReason::Stop => {}
        }
        let context = self.current_index;
        self.end_current_context();
        HostCommand::Recycle {
            context,
            reason,
            stats,
        }
    }

    /// Emit the one-shot finalize after stop. Idempotent: a second `poll` after
    /// finalize returns `None`.
    fn take_finalize(&mut self) -> Option<HostCommand> {
        // Latch: once finalized, `next_context` is left as-is and we report no
        // active context. We use a sentinel by checking whether we have already
        // finalized via `current` being cleared and state already Stopped with a
        // consumed flag. Simpler: finalize exactly once by transitioning to a
        // terminal sub-state encoded as `current_index == u32::MAX`.
        if self.current_index == FINALIZED {
            return None;
        }
        let stats = if self.current == ContextStats::default() {
            None
        } else {
            Some(self.current)
        };
        self.current = ContextStats::default();
        self.current_index = FINALIZED;
        Some(HostCommand::Finalize { stats })
    }

    fn end_current_context(&mut self) {
        self.current = ContextStats::default();
        self.state = LoopState::Idle;
    }

    /// Host event: the host started the `generate` context the policy asked for.
    /// Confirms the prompt token count (in case the host's measured prompt
    /// differs from the policy's estimate). JS `Diag.onLoopIter(inTok)`.
    pub fn on_context_started(&mut self, prompt_tokens: u32) {
        if self.state == LoopState::Running {
            self.current.prompt_tokens = prompt_tokens;
        }
    }

    /// Host event: the worker emitted `n` new tokens (a `streamer.put`). Advances
    /// the token counter and the session total. JS `Diag.onPut(nTokens)` +
    /// `ctxLen += nTokens`.
    ///
    /// Does **not** itself recycle — the policy recycles on the next [`poll`], so
    /// the host can drain a burst of tokens and the policy still applies the cap
    /// at the seam. This matches the JS, where `generate` returns (hits the cap)
    /// and only *then* does the outer `while` recycle.
    ///
    /// [`poll`]: VoxtralRecyclePolicy::poll
    pub fn on_tokens(&mut self, n: u32) {
        if self.state != LoopState::Running {
            return;
        }
        self.current.tokens_emitted = self.current.tokens_emitted.saturating_add(n);
        self.session.tokens_total = self.session.tokens_total.saturating_add(u64::from(n));
    }

    /// Host event: the context has consumed audio up to `consumed_abs` (the ring
    /// position of the last sample fed). Updates `audio_samples = consumed_abs -
    /// anchor_abs` for the active context. JS: `startIdx` advancing, measured as
    /// `startIdx - baseAbs`.
    ///
    /// `anchor_abs` is the context's anchor (echoed by the host from the
    /// [`HostCommand::StartContext`] it executed) so the policy needs no shared
    /// ring handle — it works purely from reported positions, which is what keeps
    /// it browser-free and testable.
    pub fn on_audio_advanced(&mut self, anchor_abs: u64, consumed_abs: u64) {
        if self.state != LoopState::Running {
            return;
        }
        self.current.audio_samples = consumed_abs.saturating_sub(anchor_abs);
    }

    /// Request stop (JS `isStop()` latches true). The next [`poll`] emits the
    /// single [`HostCommand::Finalize`]. Safe to call repeatedly.
    ///
    /// [`poll`]: VoxtralRecyclePolicy::poll
    pub fn request_stop(&mut self) {
        // Preserve the active context's stats for the finalize report; only flip
        // state. If we were Running, `current` already holds the live stats.
        self.state = LoopState::Stopped;
    }

    /// Feed a decoded-text delta from the host into the in-place partial-text
    /// accumulator and return the resulting text events. This is the
    /// `flushDecodedText` port; see [`PartialText`].
    ///
    /// `decoded` is the host's `tokenizer.decode(tokenCache, …)` output for the
    /// context so far (the *cumulative* decode, exactly as JS passes it). The
    /// policy owns the `printLen`/`sentenceBuffer` slicing.
    pub fn on_decoded_text(&mut self, decoded: &str) -> Vec<TextEvent> {
        if self.state != LoopState::Running {
            return Vec::new();
        }
        self.text.flush(decoded)
    }

    /// End-of-context text flush (the `streamer.end()` port): emit any trailing
    /// non-empty sentence buffer as a [`TextEvent::Final`] and reset the
    /// per-context text state. Called by the host when `generate` returns.
    pub fn on_context_end_text(&mut self) -> Vec<TextEvent> {
        self.text.end()
    }
}

/// Sentinel context index marking the finalized terminal state. `u32::MAX` is
/// unreachable as a real context count (it would require 4-billion recycles).
const FINALIZED: u32 = u32::MAX;

/// In-place partial-text accumulation: the `printLen` / `sentenceBuffer` machine
/// from `flushDecodedText` and `streamer.end()`.
///
/// The host streams the *cumulative* decode of the context's tokens
/// (`tokenizer.decode(tokenCache, { skip_special_tokens: true })`). This struct
/// owns:
///
/// - `print_len` — how many chars of that cumulative decode have already been
///   consumed, so each flush appends only the *new* tail (JS `text.slice(printLen)`).
/// - `sentence_buffer` — the in-place live text; emitted as [`TextEvent::Partial`]
///   on every non-empty delta, and sliced when a sentence completes.
///
/// Sentence boundary detection ports the JS regex `^(.*[.!?])\s*` with the `s`
/// (dotall) flag: the longest prefix ending in `.`/`!`/`?` followed by optional
/// whitespace. Implemented by hand (no regex dep) to stay byte-identical and
/// dependency-light.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PartialText {
    /// Chars of the cumulative decode already consumed (JS `printLen`).
    print_len: usize,
    /// The live in-place buffer (JS `sentenceBuffer`).
    sentence_buffer: String,
}

impl PartialText {
    fn new() -> Self {
        Self {
            print_len: 0,
            sentence_buffer: String::new(),
        }
    }

    /// Reset for a fresh context (JS, per generate context: `tokenCache = []`,
    /// `printLen = 0`, `sentenceBuffer = ''`).
    fn reset(&mut self) {
        self.print_len = 0;
        self.sentence_buffer.clear();
    }

    /// `flushDecodedText`: take the cumulative decode, append only the new tail,
    /// emit a [`TextEvent::Partial`], then peel off completed sentences as
    /// [`TextEvent::Final`].
    ///
    /// Byte-for-byte port:
    /// ```text
    /// const printableText = text.slice(printLen);
    /// printLen = text.length;
    /// if (printableText.length === 0) return;
    /// sentenceBuffer += printableText;
    /// onPartial(sentenceBuffer);
    /// const sentenceEnd = sentenceBuffer.match(/^(.*[.!?])\s*/s);
    /// if (sentenceEnd) { onFinal(sentenceEnd[1].trim()); sentenceBuffer = sentenceBuffer.slice(sentenceEnd[0].length); }
    /// ```
    fn flush(&mut self, cumulative_decoded: &str) -> Vec<TextEvent> {
        // JS uses UTF-16 code-unit indices (`String.prototype.slice`); Rust
        // strings are UTF-8. We slice on `char` boundaries to stay correct for
        // non-ASCII while matching the JS semantics for the ASCII transcript text
        // Voxtral emits. `print_len` counts chars (Unicode scalar values), and we
        // guard against a shrinking cumulative string (host re-decode quirk).
        let total_chars = cumulative_decoded.chars().count();
        if total_chars <= self.print_len {
            // Nothing new (or the decode shrank — treat as no-op, like JS where
            // `printableText.length === 0` early-returns).
            self.print_len = total_chars;
            return Vec::new();
        }
        let printable: String = cumulative_decoded.chars().skip(self.print_len).collect();
        self.print_len = total_chars;
        if printable.is_empty() {
            return Vec::new();
        }

        self.sentence_buffer.push_str(&printable);

        let mut events = vec![TextEvent::Partial(self.sentence_buffer.clone())];

        // Peel completed sentences. JS runs the regex once per flush (not in a
        // loop), so we emit at most one Final per flush — match that exactly.
        if let Some((sentence, consumed)) = Self::match_sentence(&self.sentence_buffer) {
            let trimmed = sentence.trim();
            if !trimmed.is_empty() {
                events.push(TextEvent::Final(trimmed.to_owned()));
            }
            // `sentenceBuffer = sentenceBuffer.slice(sentenceEnd[0].length)` —
            // drop the matched span (sentence + trailing whitespace).
            self.sentence_buffer = self.sentence_buffer.chars().skip(consumed).collect();
        }

        events
    }

    /// `streamer.end()`: flush the trailing buffer as a Final if non-empty, then
    /// reset. The cumulative decode is already fully consumed by the time `end`
    /// fires (the host calls `flush` first), so `end` only drains the buffer.
    ///
    /// JS:
    /// ```text
    /// if (sentenceBuffer.trim().length > 0) { onFinal(sentenceBuffer.trim()); sentenceBuffer = ''; }
    /// tokenCache = []; printLen = 0; isPrompt = true;
    /// ```
    fn end(&mut self) -> Vec<TextEvent> {
        let mut events = Vec::new();
        let trimmed = self.sentence_buffer.trim();
        if !trimmed.is_empty() {
            events.push(TextEvent::Final(trimmed.to_owned()));
        }
        self.reset();
        events
    }

    /// Port of `buffer.match(/^(.*[.!?])\s*/s)`.
    ///
    /// With the `s` (dotall) flag, `.*` is greedy across newlines, so `(.*[.!?])`
    /// captures from the start through the **last** `.`/`!`/`?` in the string,
    /// and `\s*` then consumes any whitespace after it.
    ///
    /// Returns `(captured_sentence, consumed_char_count)` where `consumed_char_count`
    /// is the length of the whole match (group-1 + trailing whitespace) in chars,
    /// i.e. `sentenceEnd[0].length`. `None` if there is no sentence-ending punct.
    fn match_sentence(buffer: &str) -> Option<(String, usize)> {
        let chars: Vec<char> = buffer.chars().collect();
        // Index (char position) of the last sentence-ending punctuation.
        let last_end = chars
            .iter()
            .rposition(|&c| c == '.' || c == '!' || c == '?')?;
        // group 1 = chars[0..=last_end]
        let sentence: String = chars[..=last_end].iter().collect();
        // trailing `\s*` after the punctuation.
        let mut consumed = last_end + 1;
        while consumed < chars.len() && chars[consumed].is_whitespace() {
            consumed += 1;
        }
        Some((sentence, consumed))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the workspace \
              lint config permits this in test code (PRD 'Rust engineering bar')"
)]
mod tests {
    use super::*;

    /// A tiny config so caps trip after a few simulated events — keeps the unit
    /// tests fast and readable while exercising the exact same code paths.
    const TINY: RecycleConfig = RecycleConfig {
        max_new_tokens: 10,
        max_ctx_samples: 16_000, // 1 s
    };

    // ---- cap decision points -------------------------------------------------

    #[test]
    fn first_poll_starts_a_context_anchored_at_ring_position() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        let cmd = p.poll(0, 3).unwrap();
        assert_eq!(
            cmd,
            HostCommand::StartContext {
                context: 0,
                anchor_abs: 0,
                max_new_tokens: 10,
            }
        );
        assert!(p.is_running());
        assert_eq!(p.current_stats().prompt_tokens, 3);
    }

    #[test]
    fn running_context_does_not_recycle_below_either_cap() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_tokens(9); // below token cap (10)
        p.on_audio_advanced(0, 15_000); // below audio cap (16_000)
        assert!(p.poll(15_000, 0).is_none(), "must keep streaming");
        assert!(p.is_running());
    }

    #[test]
    fn token_cap_triggers_recycle_with_token_reason() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_tokens(10); // exactly the cap → recycle
        let cmd = p.poll(0, 0).unwrap();
        match cmd {
            HostCommand::Recycle {
                context,
                reason,
                stats,
            } => {
                assert_eq!(context, 0);
                assert_eq!(reason, RecycleReason::TokenCap);
                assert_eq!(stats.tokens_emitted, 10);
            }
            other => panic!("expected Recycle, got {other:?}"),
        }
        // After recycle the loop is idle; next poll opens the seam context.
        assert!(!p.is_running());
        assert_eq!(p.session_stats().token_cap_recycles, 1);
    }

    #[test]
    fn audio_cap_triggers_recycle_with_audio_reason() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_tokens(2); // well below token cap
        p.on_audio_advanced(0, 16_001); // one sample past the 16_000 cap
        let cmd = p.poll(16_001, 0).unwrap();
        match cmd {
            HostCommand::Recycle { reason, stats, .. } => {
                assert_eq!(reason, RecycleReason::AudioCap);
                assert_eq!(stats.audio_samples, 16_001);
                assert_eq!(stats.tokens_emitted, 2);
            }
            other => panic!("expected Recycle, got {other:?}"),
        }
        assert_eq!(p.session_stats().audio_cap_recycles, 1);
    }

    #[test]
    fn audio_cap_boundary_is_strictly_greater_than() {
        // JS: `if (startIdx - baseAbs > MAX_CTX_SAMPLES) break;` — strictly `>`.
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_audio_advanced(0, 16_000); // exactly the cap → NOT yet
        assert!(p.poll(16_000, 0).is_none());
        p.on_audio_advanced(0, 16_001); // one past → recycle
        assert!(matches!(
            p.poll(16_001, 0),
            Some(HostCommand::Recycle {
                reason: RecycleReason::AudioCap,
                ..
            })
        ));
    }

    #[test]
    fn token_cap_boundary_is_greater_or_equal() {
        // `generate` returns when it has produced `max_new_tokens` — reaching the
        // cap (>=) ends the context.
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_tokens(9);
        assert!(p.poll(0, 0).is_none(), "9 < 10, keep going");
        p.on_tokens(1); // now 10
        assert!(matches!(
            p.poll(0, 0),
            Some(HostCommand::Recycle {
                reason: RecycleReason::TokenCap,
                ..
            })
        ));
    }

    #[test]
    fn audio_cap_wins_when_both_caps_trip_in_same_context() {
        // Cap precedence: audio/time cap is the primary guard (JS evaluates it in
        // the mel generator before feeding), so it wins over a simultaneous token
        // cap.
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_tokens(10); // token cap reached
        p.on_audio_advanced(0, 20_000); // audio cap also reached
        assert!(matches!(
            p.poll(20_000, 0),
            Some(HostCommand::Recycle {
                reason: RecycleReason::AudioCap,
                ..
            })
        ));
    }

    // ---- seam: recycle preserves context correctly ---------------------------

    #[test]
    fn recycle_anchors_seam_context_at_current_ring_position_no_skip_no_reread() {
        // The defining property: a recycled context anchors at the CURRENT ring
        // write position — never re-reading evicted samples, never skipping audio.
        let mut p = VoxtralRecyclePolicy::new(TINY);
        // Context 0 anchored at 0, runs to the token cap while the ring advances
        // to 50_000.
        assert_eq!(
            p.poll(0, 0).unwrap(),
            HostCommand::StartContext {
                context: 0,
                anchor_abs: 0,
                max_new_tokens: 10
            }
        );
        p.on_tokens(10);
        p.on_audio_advanced(0, 40_000);
        // Recycle (context 0 ends).
        assert!(matches!(
            p.poll(50_000, 0),
            Some(HostCommand::Recycle { .. })
        ));
        // Seam: next poll starts context 1 anchored at the CURRENT ring position
        // (50_000), not at where context 0 ended reading (40_000) — forward from
        // "now", continuous, no gap.
        assert_eq!(
            p.poll(50_000, 5).unwrap(),
            HostCommand::StartContext {
                context: 1,
                anchor_abs: 50_000,
                max_new_tokens: 10
            }
        );
        assert_eq!(p.current_stats().prompt_tokens, 5);
        // Per-context stats reset across the seam (the bounded sawtooth).
        assert_eq!(p.current_stats().tokens_emitted, 0);
        assert_eq!(p.current_stats().audio_samples, 0);
    }

    #[test]
    fn per_context_stats_reset_on_every_recycle() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        for ctx in 0..3u32 {
            let started = p.poll(u64::from(ctx) * 100_000, 0).unwrap();
            assert!(matches!(
                started,
                HostCommand::StartContext { context, .. } if context == ctx
            ));
            // Fill to the token cap.
            p.on_tokens(10);
            assert!(matches!(p.poll(0, 0), Some(HostCommand::Recycle { .. })));
            // Stats cleared after the recycle.
            assert_eq!(p.current_stats().tokens_emitted, 0);
        }
        assert_eq!(p.session_stats().contexts_started, 3);
        assert_eq!(p.session_stats().recycles, 3);
    }

    // ---- stop / finalize -----------------------------------------------------

    #[test]
    fn stop_emits_one_finalize_then_none() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_tokens(4);
        p.request_stop();
        let cmd = p.poll(0, 0).unwrap();
        match cmd {
            HostCommand::Finalize { stats } => {
                let s = stats.expect("active context stats reported at finalize");
                assert_eq!(s.tokens_emitted, 4);
            }
            other => panic!("expected Finalize, got {other:?}"),
        }
        // Idempotent: no second finalize, no further commands.
        assert!(p.poll(0, 0).is_none());
        assert!(p.poll(0, 0).is_none());
    }

    #[test]
    fn events_ignored_after_stop() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.request_stop();
        // Host events after stop are no-ops (JS: streamer.put/end early-return on isStop).
        p.on_tokens(100);
        p.on_audio_advanced(0, 999_999);
        assert!(p.on_decoded_text("ignored").is_empty());
    }

    // ---- in-place partial text (flushDecodedText / streamer.end) -------------

    #[test]
    fn partial_text_emits_in_place_partials_for_each_delta() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        // Cumulative decode grows: "Hel" → "Hello" (in-place, no sentence yet).
        assert_eq!(
            p.on_decoded_text("Hel"),
            vec![TextEvent::Partial("Hel".into())]
        );
        assert_eq!(
            p.on_decoded_text("Hello"),
            vec![TextEvent::Partial("Hello".into())]
        );
        // No new chars → no event (JS `printableText.length === 0` early return).
        assert_eq!(p.on_decoded_text("Hello"), vec![]);
    }

    #[test]
    fn partial_text_promotes_completed_sentence_to_final_and_keeps_remainder() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        // "Hello world. And mo" → Final("Hello world.") + buffer keeps "And mo".
        let ev = p.on_decoded_text("Hello world. And mo");
        assert_eq!(
            ev,
            vec![
                TextEvent::Partial("Hello world. And mo".into()),
                TextEvent::Final("Hello world.".into()),
            ]
        );
        // Next delta continues the remainder in place.
        let ev2 = p.on_decoded_text("Hello world. And more text");
        assert_eq!(ev2, vec![TextEvent::Partial("And more text".into())]);
    }

    #[test]
    fn partial_text_dotall_greedy_captures_through_last_punctuation() {
        // JS `/^(.*[.!?])\s*/s`: greedy `.*` with dotall captures through the LAST
        // sentence-ending punctuation, across newlines.
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        let ev = p.on_decoded_text("One. Two!\nThree? rest");
        assert_eq!(
            ev,
            vec![
                TextEvent::Partial("One. Two!\nThree? rest".into()),
                TextEvent::Final("One. Two!\nThree?".into()),
            ]
        );
    }

    #[test]
    fn context_end_flushes_trailing_buffer_as_final() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_decoded_text("no terminal punctuation here");
        // streamer.end(): trailing non-empty buffer becomes a Final.
        assert_eq!(
            p.on_context_end_text(),
            vec![TextEvent::Final("no terminal punctuation here".into())]
        );
        // Buffer reset after end.
        assert_eq!(p.on_context_end_text(), vec![]);
    }

    #[test]
    fn text_state_resets_across_recycle_seam() {
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        p.on_decoded_text("partial without end");
        p.on_tokens(10);
        // Recycle: the JS resets tokenCache/printLen/sentenceBuffer for the fresh
        // context. The trailing buffer from the old context is NOT silently
        // carried into the new one's partials.
        assert!(matches!(p.poll(0, 0), Some(HostCommand::Recycle { .. })));
        p.poll(1000, 0); // seam context
        // Fresh context's first decode starts from an empty buffer/print_len.
        assert_eq!(
            p.on_decoded_text("New."),
            vec![
                TextEvent::Partial("New.".into()),
                TextEvent::Final("New.".into()),
            ]
        );
    }

    #[test]
    fn unicode_decode_slices_on_char_boundaries() {
        // Non-ASCII must not panic and must slice correctly (JS uses UTF-16 code
        // units; we use char counts — correct for the scalar-value transcript text).
        let mut p = VoxtralRecyclePolicy::new(TINY);
        p.poll(0, 0);
        assert_eq!(
            p.on_decoded_text("café"),
            vec![TextEvent::Partial("café".into())]
        );
        // Append "café résumé." cumulative → delta " résumé." then sentence Final.
        let ev = p.on_decoded_text("café résumé.");
        assert_eq!(
            ev,
            vec![
                TextEvent::Partial("café résumé.".into()),
                TextEvent::Final("café résumé.".into()),
            ]
        );
    }

    // ---- 10-minute session simulation: bounded context growth ----------------

    /// The headline acceptance: a 10-minute session under the **shipping** caps
    /// shows bounded per-context growth (the sawtooth) and no unbounded
    /// accumulation — the runaway-RAM bug, made into a regression test (PRD R9:
    /// *"Voxtral 10-minute session: flat memory (two-cap recycle)"*).
    #[test]
    fn ten_minute_session_bounded_context_growth() {
        // Simulated streams. We drive a realistic-ish profile: speech at a sparse
        // ~4 tok/s and audio advancing in real time. At 4 tok/s the token cap (320)
        // would take ~80 s, but the 45 s audio cap fires first — so this is a
        // *slow-token* profile, the exact case the audio cap exists for.
        const TOTAL_MS: u64 = 10 * 60 * 1000; // 10 minutes
        const STEP_MS: u64 = 250; // host feeds a chunk every 250 ms (Nemotron-like cadence)
        const SAMPLES_PER_STEP: u64 = SAMPLE_RATE_HZ * STEP_MS / 1000; // 4000
        // 1 token per 250 ms step = 4 tok/s: a sparse "slow-token" profile where
        // the 45 s audio cap fires long before the 320-token cap.
        const TOKENS_PER_STEP: u32 = 1;

        let cfg = RecycleConfig::VOXTRAL_SHIPPING;
        let mut p = VoxtralRecyclePolicy::new(cfg);

        // Pull the anchor out of a `StartContext` command (panics on any other) —
        // used to prime the first context and re-anchor after each recycle.
        let anchor_of = |cmd: Option<HostCommand>| -> u64 {
            match cmd {
                Some(HostCommand::StartContext { anchor_abs, .. }) => anchor_abs,
                other => panic!("expected StartContext, got {other:?}"),
            }
        };

        let mut ring_write_abs: u64 = 0;
        let mut max_ctx_tokens_seen: u32 = 0;
        let mut max_ctx_samples_seen: u64 = 0;
        let mut elapsed_ms: u64 = 0;
        let mut step: u64 = 0;

        // Prime the first context.
        let mut anchor_abs = anchor_of(p.poll(ring_write_abs, 2));

        while elapsed_ms < TOTAL_MS {
            // The ring advances by one chunk of real audio.
            ring_write_abs += SAMPLES_PER_STEP;
            elapsed_ms += STEP_MS;
            step += 1;

            // Host reports progress to the active context.
            if p.is_running() {
                p.on_tokens(TOKENS_PER_STEP);
                p.on_audio_advanced(anchor_abs, ring_write_abs);
                max_ctx_tokens_seen = max_ctx_tokens_seen.max(p.current_stats().tokens_emitted);
                max_ctx_samples_seen = max_ctx_samples_seen.max(p.current_stats().audio_samples);
            }

            // Drive the policy. On a recycle, immediately open the seam context
            // (the outer-while in the JS does exactly this) and capture its anchor.
            match p.poll(ring_write_abs, 2) {
                Some(HostCommand::Recycle { reason, stats, .. }) => {
                    // Every recycle's stats are within the caps — the invariant.
                    assert!(
                        stats.tokens_emitted <= cfg.max_new_tokens,
                        "context exceeded token cap: {} > {}",
                        stats.tokens_emitted,
                        cfg.max_new_tokens
                    );
                    // Audio cap is strictly-greater, so a capped context is at most
                    // one chunk past the cap.
                    assert!(
                        stats.audio_samples <= cfg.max_ctx_samples + SAMPLES_PER_STEP,
                        "context exceeded audio cap by >1 chunk: {} > {}",
                        stats.audio_samples,
                        cfg.max_ctx_samples
                    );
                    // In this slow-token profile the audio cap is what fires.
                    assert_eq!(reason, RecycleReason::AudioCap);
                    // Open the seam context anchored at the current ring position.
                    anchor_abs = anchor_of(p.poll(ring_write_abs, 2));
                }
                Some(HostCommand::StartContext { .. }) => {
                    panic!("unexpected StartContext without a preceding Recycle")
                }
                Some(HostCommand::Finalize { .. }) | None => {}
            }
        }

        let s = p.session_stats();

        // --- bounded growth assertions (the whole point) ---

        // Per-context token count NEVER exceeded the token cap.
        assert!(
            max_ctx_tokens_seen <= cfg.max_new_tokens,
            "per-context tokens unbounded: peak {max_ctx_tokens_seen} > cap {}",
            cfg.max_new_tokens
        );
        // Per-context audio NEVER exceeded the audio cap by more than one chunk.
        assert!(
            max_ctx_samples_seen <= cfg.max_ctx_samples + SAMPLES_PER_STEP,
            "per-context audio unbounded: peak {max_ctx_samples_seen} > cap {}",
            cfg.max_ctx_samples
        );

        // The session DID recycle repeatedly (proving the loop ran, not that it
        // simply never hit a cap). 10 min / 45 s ≈ 13 audio-cap recycles.
        assert!(
            s.recycles >= 10,
            "expected many recycles over 10 min, got {}",
            s.recycles
        );
        assert!(s.audio_cap_recycles >= 10);
        assert_eq!(
            s.contexts_started,
            s.recycles + 1,
            "one open context remains"
        );

        // Sanity on the simulated duration: ~2400 steps over 10 min.
        assert_eq!(step, TOTAL_MS / STEP_MS);

        // Clean stop.
        p.request_stop();
        assert!(matches!(
            p.poll(ring_write_abs, 0),
            Some(HostCommand::Finalize { .. })
        ));
    }

    /// A *fast-token* profile (continuous dense speech) where the TOKEN cap fires
    /// before the audio cap — the complementary case. Still bounded.
    #[test]
    fn ten_minute_session_fast_tokens_hits_token_cap_and_stays_bounded() {
        const TOTAL_STEPS: u64 = 2400; // 10 min @ 250 ms
        const SAMPLES_PER_STEP: u64 = 4000;
        // ~12 tokens per 250 ms step = 48 tok/s (dense): token cap (320) fires at
        // ~27 steps ≈ 6.7 s, long before the 45 s audio cap.
        const TOKENS_PER_STEP: u32 = 12;

        let cfg = RecycleConfig::VOXTRAL_SHIPPING;
        let mut p = VoxtralRecyclePolicy::new(cfg);

        let anchor_of = |cmd: Option<HostCommand>| -> u64 {
            match cmd {
                Some(HostCommand::StartContext { anchor_abs, .. }) => anchor_abs,
                other => panic!("expected StartContext, got {other:?}"),
            }
        };

        let mut ring: u64 = 0;
        let mut peak_tokens: u32 = 0;
        let mut anchor = anchor_of(p.poll(ring, 2));

        for _ in 0..TOTAL_STEPS {
            ring += SAMPLES_PER_STEP;
            if p.is_running() {
                p.on_tokens(TOKENS_PER_STEP);
                p.on_audio_advanced(anchor, ring);
                peak_tokens = peak_tokens.max(p.current_stats().tokens_emitted);
            }
            if let Some(HostCommand::Recycle { reason, stats, .. }) = p.poll(ring, 2) {
                assert_eq!(reason, RecycleReason::TokenCap);
                // A token-capped context overshoots by at most one host burst.
                assert!(stats.tokens_emitted <= cfg.max_new_tokens + TOKENS_PER_STEP);
                anchor = anchor_of(p.poll(ring, 2));
            }
        }

        let s = p.session_stats();
        assert!(s.token_cap_recycles >= 50, "got {}", s.token_cap_recycles);
        assert_eq!(s.audio_cap_recycles, 0, "audio cap should not fire here");
        // Peak per-context tokens bounded near the cap (not unbounded).
        assert!(peak_tokens <= cfg.max_new_tokens + TOKENS_PER_STEP);
    }

    // ---- serialization (the typed boundary to the JS host) -------------------

    #[test]
    fn host_command_serializes_as_discriminated_union() {
        // The JS host consumes `{ cmd: "...", ... }` (the b2 spike's tag shape).
        let start = HostCommand::StartContext {
            context: 0,
            anchor_abs: 1234,
            max_new_tokens: 320,
        };
        let j = serde_json::to_value(&start).unwrap();
        assert_eq!(j["cmd"], "start_context");
        assert_eq!(j["anchor_abs"], 1234);
        assert_eq!(j["max_new_tokens"], 320);

        let recycle = HostCommand::Recycle {
            context: 1,
            reason: RecycleReason::AudioCap,
            stats: ContextStats {
                tokens_emitted: 7,
                audio_samples: 720_001,
                prompt_tokens: 2,
            },
        };
        let j = serde_json::to_value(&recycle).unwrap();
        assert_eq!(j["cmd"], "recycle");
        assert_eq!(j["reason"], "audio_cap");
        assert_eq!(j["stats"]["tokens_emitted"], 7);

        // Round-trips.
        let back: HostCommand = serde_json::from_value(j).unwrap();
        assert_eq!(back, recycle);
    }

    #[test]
    fn text_event_serializes_for_the_ui_boundary() {
        let p = TextEvent::Partial("live text".into());
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({ "kind": "partial", "text": "live text" })
        );
        let f = TextEvent::Final("done.".into());
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({ "kind": "final", "text": "done." })
        );
    }

    #[test]
    fn shipping_config_matches_the_js_constants() {
        let c = RecycleConfig::VOXTRAL_SHIPPING;
        assert_eq!(c.max_new_tokens, 320); // JS `max_new_tokens: 320`
        assert_eq!(c.max_ctx_samples, 16_000 * 45); // JS `MAX_CTX_SAMPLES`
        assert!((c.max_ctx_seconds() - 45.0).abs() < 1e-9);
    }
}
