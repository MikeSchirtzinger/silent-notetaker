//! Whisper-family (and Moonshine) streaming-loop policy, as a Rust policy module.
//!
//! This ports the transformers.js transcription worker + its main-thread feeder
//! (`index.html`'s inlined `transcription-worker-src` and `startMoonshine`) into
//! deterministic, browser-free Rust policy (PRD R2, Phase 5; Appendix A rows 7,
//! 11). The same loop runs every transformers.js ASR model that is **final-only**
//! in the worker: Whisper large-turbo / small.en / base.en / tiny.en, and
//! Moonshine. (Voxtral is *not* this loop — it streams in place with the two-cap
//! recycle, ported in [`crate::voxtral_recycle`].)
//!
//! # What the JS worker did, and what is policy
//!
//! The worker is a fixed pipeline applied to each fed audio chunk
//! (`processQueue` in `index.html`):
//!
//! 1. **Chunking** (main thread, `startMoonshine`): accumulate 16 kHz samples in a
//!    buffer and, once `CHUNK_SAMPLES` (16000 × 5 s solo; 16000 × 3 s for Moonshine
//!    in Dual) have arrived, `splice` off exactly one chunk and post it to the
//!    worker. Leftover samples stay buffered for the next chunk — **no overlap, no
//!    drop**.
//! 2. **VAD gate** (`hasSpeech`): a strided RMS energy check against
//!    `vadThreshold` (default `0.008`). Below threshold → the chunk is **skipped**
//!    entirely (no decode, no event).
//! 3. **Decode** → text (the model runs in the host; this is the executor step).
//! 4. **Hallucination filter** (`isHallucination`): drop known Whisper junk
//!    ("thank you for watching", a lone "you", a 4+-word line that is all one
//!    repeated word, …).
//! 5. **Tail dedup** (`deduplicateText`, `DEDUP_WINDOW = 12`): chunk boundaries
//!    re-emit a few leading words from the previous chunk's tail; strip the
//!    longest leading run that the previous tail already ended with / contains.
//! 6. **Corrections**: `applyCorrections` — already ported to Rust
//!    (`silent-notes` `Corrections`, surfaced as `WasmCorrections`); **not**
//!    duplicated here. This module emits the post-dedup text; corrections compose
//!    downstream exactly where the JS applied them (after dedup, before emit).
//! 7. **Emit**: a non-empty result is posted as a `final` message; the worker also
//!    records the emitted tail as the new dedup state.
//!
//! Steps 1–2 and 4–5 (and 7's dedup-state update) are **decisions** — when to feed
//! a chunk, whether it is speech, whether it is a hallucination, what to strip as a
//! duplicate. They are pure functions of the audio/text stream and live here as
//! tested Rust law. Step 3 (the model) and step 6 (corrections, already Rust) are
//! the executor's job; this policy emits a typed [`HostCommand::Transcribe`] for
//! the chunk and consumes the host's decoded text via [`WhisperStreamPolicy::on_decoded`].
//!
//! # Final-only, by design
//!
//! The transformers.js worker emits **only** `final` messages — there is no
//! in-place partial inside the loop (the chunk *is* the unit). So this policy emits
//! [`TextEvent::Final`] only. (In Dual mode the *consumer* renders Moonshine's
//! finals as drafts; that re-labeling is the Dual coordinator's job, see
//! [`crate::dual`], not this loop's.)
//!
//! No `unwrap`/`expect` on any path; no fallible operation on the hot path
//! (PRD "Rust engineering bar").

use serde::{Deserialize, Serialize};

use crate::TextEvent;

/// 16 kHz mono is the rate every engine expects. Used for sample→ms conversion.
const SAMPLE_RATE_HZ: u64 = 16_000;

/// The same rate as a `usize`, for sample-count (chunk-size) arithmetic without a
/// `u64 as usize` cast in `const` context.
const SAMPLE_RATE_HZ_USIZE: usize = 16_000;

/// The dedup look-back window: at most this many leading words of a chunk are
/// considered duplicates of the previous chunk's tail (JS `DEDUP_WINDOW = 12`).
const DEDUP_WINDOW: usize = 12;

/// Configuration for the streaming loop, mirroring the JS worker `config` + the
/// main-thread `CHUNK_SAMPLES`.
///
/// In the running app these arrive as registry config (PRD R4 / Task I3); here
/// they are explicit so the policy can be pinned to the exact shipping values and
/// so tests can shrink them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WhisperStreamConfig {
    /// Samples per chunk handed to the host (JS `CHUNK_SAMPLES`). 16000 × 5 for
    /// Whisper/Moonshine solo; 16000 × 3 for Moonshine in Dual mode.
    pub chunk_samples: usize,
    /// RMS energy threshold below which a chunk is treated as silence and skipped
    /// (JS `vadThreshold`, default `0.008`).
    pub vad_threshold: f32,
}

impl WhisperStreamConfig {
    /// The shipping **solo** chunk size: 5 s at 16 kHz (`16000 * 5 = 80_000`).
    pub const SOLO_CHUNK_SAMPLES: usize = SAMPLE_RATE_HZ_USIZE * 5;
    /// The shipping **Moonshine-in-Dual** chunk size: 3 s (`16000 * 3 = 48_000`).
    pub const DUAL_MOONSHINE_CHUNK_SAMPLES: usize = SAMPLE_RATE_HZ_USIZE * 3;
    /// The shipping VAD threshold (JS worker `config.vadThreshold`).
    pub const SHIPPING_VAD_THRESHOLD: f32 = 0.008;

    /// Whisper / Moonshine **solo**: 5 s chunks, shipping VAD threshold.
    pub const WHISPER_SOLO: WhisperStreamConfig = WhisperStreamConfig {
        chunk_samples: Self::SOLO_CHUNK_SAMPLES,
        vad_threshold: Self::SHIPPING_VAD_THRESHOLD,
    };

    /// Moonshine **in Dual mode**: 3 s chunks (faster draft feedback), shipping
    /// VAD threshold.
    pub const MOONSHINE_DUAL: WhisperStreamConfig = WhisperStreamConfig {
        chunk_samples: Self::DUAL_MOONSHINE_CHUNK_SAMPLES,
        vad_threshold: Self::SHIPPING_VAD_THRESHOLD,
    };
}

impl Default for WhisperStreamConfig {
    fn default() -> Self {
        Self::WHISPER_SOLO
    }
}

/// A typed command the policy emits for the JS host (transformers.js worker) to
/// execute. `#[serde(tag = "cmd")]` gives the JS side a discriminated union.
///
/// `#[non_exhaustive]` so adding a command does not break the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostCommand {
    /// Run the host's `transcriber(audio, …)` on exactly one chunk of audio. The
    /// host replies with decoded text, which the policy threads through dedup /
    /// hallucination filtering via [`WhisperStreamPolicy::on_decoded`]. JS:
    /// `worker.postMessage({ type: 'transcribe', audio: chunk })`.
    Transcribe {
        /// Monotonic chunk index, so events correlate with the chunk that produced
        /// them (the JS worker is FIFO; this makes the ordering explicit + testable).
        chunk: u32,
        /// The audio span this chunk covers, in ms from session start. Lets the
        /// host (and the UI) stamp the emitted text without a shared clock.
        start_ms: u64,
        /// End of the span (exclusive), ms from session start.
        end_ms: u64,
    },
    /// End of stream: the host should tear down the worker. JS:
    /// `worker.postMessage({ type: 'terminate' })`.
    Finalize,
}

/// Why a fed chunk produced no [`HostCommand::Transcribe`] — i.e. the policy
/// dropped it before the host ever ran. Surfaced so tests (and diagnostics) can
/// assert *which* gate fired, mirroring the JS `continue` in `processQueue`.
///
/// `#[non_exhaustive]` for additive gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SkipReason {
    /// The chunk's RMS energy was at or below the VAD threshold (JS `!hasSpeech`).
    Silence,
}

/// The Whisper-family / Moonshine streaming-loop policy.
///
/// Drive it by pushing captured 16 kHz samples ([`push_samples`]); it emits a
/// [`HostCommand::Transcribe`] each time a whole chunk is ready and survives the
/// VAD gate. Feed the host's decoded text back via [`on_decoded`] to get the
/// post-filter [`TextEvent`]s. The policy decides chunk boundaries, silence,
/// hallucinations, and tail-dedup; the host only runs the model.
///
/// [`push_samples`]: WhisperStreamPolicy::push_samples
/// [`on_decoded`]: WhisperStreamPolicy::on_decoded
#[derive(Debug, Clone)]
pub struct WhisperStreamPolicy {
    config: WhisperStreamConfig,
    /// Accumulator for the main-thread chunker (JS `audioBuffer`). Holds the
    /// leftover < `chunk_samples` samples between chunks.
    buffer: Vec<f32>,
    /// Monotonic chunk index (JS chunks are implicitly ordered; we make it explicit).
    next_chunk: u32,
    /// Absolute sample position of the start of `buffer` (ms-stamping anchor): how
    /// many samples have already been spliced into completed chunks.
    consumed_samples: u64,
    /// Previous chunk's emitted tail words, lower-cased look-back for dedup
    /// (JS `prevTailWords`). Reset on [`reset`](WhisperStreamPolicy::reset).
    prev_tail_words: Vec<String>,
    /// Latched once stop is requested; further pushes are ignored and the next
    /// [`drain_finalize`](WhisperStreamPolicy::drain_finalize) emits the lone
    /// [`HostCommand::Finalize`].
    stopped: bool,
    /// Whether [`HostCommand::Finalize`] has already been emitted (idempotency).
    finalized: bool,
}

impl WhisperStreamPolicy {
    /// Build a policy with the given config. Use [`WhisperStreamConfig::WHISPER_SOLO`]
    /// for the exact solo behavior, or [`WhisperStreamConfig::MOONSHINE_DUAL`] for
    /// Moonshine's faster Dual cadence.
    #[must_use]
    pub fn new(config: WhisperStreamConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
            next_chunk: 0,
            consumed_samples: 0,
            prev_tail_words: Vec::new(),
            stopped: false,
            finalized: false,
        }
    }

    /// The config in force.
    #[must_use]
    pub fn config(&self) -> WhisperStreamConfig {
        self.config
    }

    /// Samples buffered awaiting a whole chunk (the JS leftover `audioBuffer.length`).
    #[must_use]
    pub fn pending_samples(&self) -> usize {
        self.buffer.len()
    }

    /// Push captured 16 kHz samples and emit a [`HostCommand::Transcribe`] for
    /// every whole chunk that is now ready **and** carries speech.
    ///
    /// This is the JS main-thread feeder (`startMoonshine`'s `_startAudioCapture`
    /// callback): append to the buffer, then, while at least `chunk_samples` are
    /// buffered, splice one chunk off the front. Each spliced chunk runs the VAD
    /// gate ([`hasSpeech`]); a chunk that fails is dropped (no command) — exactly
    /// the JS `if (!hasSpeech(audio, …)) continue;` in `processQueue`, hoisted to
    /// the feed step so a silent chunk never reaches the host at all (the host's
    /// FIFO queue stays empty for it, identical observable behavior).
    ///
    /// Returns `(commands, skips)`: the transcribe commands to issue, and the
    /// dropped chunks with their [`SkipReason`] (for diagnostics/tests).
    ///
    /// [`hasSpeech`]: WhisperStreamPolicy::has_speech
    pub fn push_samples(&mut self, samples: &[f32]) -> (Vec<HostCommand>, Vec<SkipReason>) {
        let mut commands = Vec::new();
        let mut skips = Vec::new();
        if self.stopped {
            return (commands, skips);
        }
        self.buffer.extend_from_slice(samples);

        while self.buffer.len() >= self.config.chunk_samples {
            // `splice(0, CHUNK_SAMPLES)`: take exactly one chunk off the front,
            // leftover stays buffered.
            let chunk: Vec<f32> = self.buffer.drain(..self.config.chunk_samples).collect();
            let chunk_len = chunk.len() as u64;
            let start_abs = self.consumed_samples;
            self.consumed_samples += chunk_len;

            if !Self::has_speech(&chunk, self.config.vad_threshold) {
                // JS `continue;` — the silent chunk is consumed but never decoded.
                skips.push(SkipReason::Silence);
                continue;
            }

            let chunk_idx = self.next_chunk;
            self.next_chunk += 1;
            commands.push(HostCommand::Transcribe {
                chunk: chunk_idx,
                start_ms: Self::samples_to_ms(start_abs),
                end_ms: Self::samples_to_ms(start_abs + chunk_len),
            });
        }

        (commands, skips)
    }

    /// Consume the host's decoded text for one chunk and apply the post-decode
    /// filters, returning the [`TextEvent`]s to surface.
    ///
    /// Byte-for-byte port of the `processQueue` tail (`index.html`):
    /// ```text
    /// let text = result.text.trim();
    /// if (text.length > 1 && !isHallucination(text)) {
    ///   text = deduplicateText(text);
    ///   // applyCorrections(text) — composed downstream (already-Rust Corrections)
    ///   if (text.trim().length > 0) {
    ///     prevTailWords = text.split(/\s+/).slice(-DEDUP_WINDOW);
    ///     postMessage({ type: 'final', text: text.trim() });
    ///   }
    /// }
    /// ```
    ///
    /// `start_ms`/`end_ms` echo the chunk's span (from the [`HostCommand::Transcribe`]
    /// the host executed) so the emitted [`TextEvent::Final`] can be range-stamped
    /// by the caller; this policy returns the text only — range attachment is the
    /// `EngineEvent` adapter's job (it has the `TimeRange`).
    pub fn on_decoded(&mut self, decoded: &str) -> Vec<TextEvent> {
        if self.stopped {
            return Vec::new();
        }
        // `result.text.trim()`, then `text.length > 1` (JS uses UTF-16 `.length`;
        // a 1-char string is dropped — we count chars, agreeing for the ASCII
        // transcript text Whisper emits and staying correct for non-ASCII).
        let trimmed = decoded.trim();
        if trimmed.chars().count() <= 1 {
            return Vec::new();
        }
        if Self::is_hallucination(trimmed) {
            return Vec::new();
        }

        let deduped = self.deduplicate(trimmed);
        // `if (text.trim().length > 0)`.
        let final_text = deduped.trim();
        if final_text.is_empty() {
            return Vec::new();
        }

        // `prevTailWords = words.slice(-DEDUP_WINDOW)` — the new dedup state is the
        // last DEDUP_WINDOW words of the *emitted* (post-dedup) text. JS splits the
        // pre-trim `text` (which already equals the deduped string here) on `\s+`.
        self.prev_tail_words = Self::last_words_lower(final_text, DEDUP_WINDOW);

        vec![TextEvent::Final(final_text.to_owned())]
    }

    /// Request stop (JS worker `terminate`). The next
    /// [`drain_finalize`](WhisperStreamPolicy::drain_finalize) emits the single
    /// [`HostCommand::Finalize`]. After stop, pushes and decodes are no-ops. Safe
    /// to call repeatedly.
    pub fn request_stop(&mut self) {
        self.stopped = true;
    }

    /// Emit the one-shot [`HostCommand::Finalize`] after stop (idempotent). The JS
    /// worker has no trailing-buffer flush — leftover `audioBuffer` (< one chunk)
    /// is simply dropped on `terminate` — so finalize emits *only* the teardown
    /// command and never a final partial chunk. Returns `None` before stop or after
    /// the finalize has already been emitted.
    pub fn drain_finalize(&mut self) -> Option<HostCommand> {
        if !self.stopped || self.finalized {
            return None;
        }
        self.finalized = true;
        Some(HostCommand::Finalize)
    }

    /// Clear all streaming state for a fresh utterance/session (JS worker `init`
    /// resets `pendingAudioChunks` / `prevTailWords`; the main thread resets
    /// `audioBuffer`). Keeps the config.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.next_chunk = 0;
        self.consumed_samples = 0;
        self.prev_tail_words.clear();
        self.stopped = false;
        self.finalized = false;
    }

    // ---- pure ports ---------------------------------------------------------

    /// `hasSpeech(audio, threshold)` — strided RMS energy gate.
    ///
    /// JS:
    /// ```text
    /// let sumSquares = 0; const step = 4; let count = 0;
    /// for (let i = 0; i < audio.length; i += step) { sumSquares += audio[i]*audio[i]; count++; }
    /// const rms = Math.sqrt(sumSquares / count);
    /// return rms > threshold;
    /// ```
    /// Samples every 4th sample (the `step = 4` stride) for speed. An empty chunk
    /// has `count === 0` → `sumSquares/count` is `NaN` → `NaN > threshold` is
    /// `false` in JS; we return `false` for the empty case to match.
    #[must_use]
    pub fn has_speech(audio: &[f32], threshold: f32) -> bool {
        const STEP: usize = 4;
        if audio.is_empty() {
            return false;
        }
        let mut sum_squares = 0.0f64;
        // The number of strided samples (every 4th) over a non-empty slice — the
        // JS `count`. `div_ceil` gives exactly the count of `i = 0, 4, 8, …` indices
        // that are `< len`, with no per-iteration counter to cast.
        let count = audio.len().div_ceil(STEP);
        let mut i = 0usize;
        while i < audio.len() {
            let s = f64::from(audio[i]);
            sum_squares += s * s;
            i += STEP;
        }
        // count >= 1 here (audio non-empty), so no division by zero. `count` is a
        // strided sample count, far under 2^52, so the usize→f64 conversion is exact.
        #[allow(
            clippy::cast_precision_loss,
            reason = "count is a strided sample count over one chunk (at most ~120k \
                      for a 30 s chunk), far below f64's exact-integer range (2^52); \
                      the conversion is lossless and reproduces the JS `sumSquares / count`."
        )]
        let count_f = count as f64;
        let rms = (sum_squares / count_f).sqrt();
        rms > f64::from(threshold)
    }

    /// `isHallucination(text)` — drop known Whisper junk lines.
    ///
    /// JS:
    /// ```text
    /// const lower = text.toLowerCase();
    /// if (HALLUCINATIONS.includes(lower) || HALLUCINATIONS.includes(lower.replace(/[.!?,]/g, ''))) return true;
    /// const words = lower.split(/\s+/);
    /// if (words.length >= 4) { const unique = new Set(words); if (unique.size === 1) return true; }
    /// return false;
    /// ```
    /// The repeated-word check catches a 4+-word line that is a single word
    /// repeated (a common Whisper degenerate loop).
    #[must_use]
    pub fn is_hallucination(text: &str) -> bool {
        // The exact 11-entry JS `hallucinations` array, in order.
        const HALLUCINATIONS: [&str; 11] = [
            "thank you for watching",
            "thanks for watching",
            "subscribe to my channel",
            "please like and subscribe",
            "thank you for listening",
            "thanks for listening",
            "you",
            "...",
            "the end",
            "bye",
            "goodbye",
        ];
        let lower = text.to_lowercase();
        // Exact-list membership (the JS `.includes(lower)`).
        let in_list = |s: &str| HALLUCINATIONS.contains(&s);

        if in_list(&lower) {
            return true;
        }
        // `lower.replace(/[.!?,]/g, '')` — strip sentence/comma punctuation.
        let stripped: String = lower
            .chars()
            .filter(|c| !matches!(c, '.' | '!' | '?' | ','))
            .collect();
        if in_list(&stripped) {
            return true;
        }

        // `lower.split(/\s+/)`: JS split on the `\s+` regex yields a leading "" for
        // leading whitespace; `lower` here is the already-trimmed text, so no
        // leading empty. We split on ASCII/Unicode whitespace runs, dropping empty
        // fragments, matching the trimmed-input behavior.
        let words: Vec<&str> = lower.split_whitespace().collect();
        if words.len() >= 4 {
            let unique: std::collections::HashSet<&str> = words.iter().copied().collect();
            if unique.len() == 1 {
                return true;
            }
        }
        false
    }

    /// `deduplicateText(text)` — strip the longest leading word-run that the
    /// previous chunk's tail already produced.
    ///
    /// JS:
    /// ```text
    /// if (prevTailWords.length === 0) return text;
    /// const words = text.split(/\s+/);
    /// const prevTail = prevTailWords.join(' ').toLowerCase();
    /// let bestCut = 0;
    /// for (let len = 1; len <= Math.min(words.length, DEDUP_WINDOW); len++) {
    ///   const candidate = words.slice(0, len).join(' ').toLowerCase();
    ///   if (prevTail.endsWith(candidate) || prevTail.includes(candidate)) bestCut = len;
    /// }
    /// if (bestCut > 0) return words.slice(bestCut).join(' ');
    /// return text;
    /// ```
    /// `bestCut` is the **largest** `len` (the loop overwrites, keeping the last
    /// match) whose first-`len`-words join is a substring of the previous tail.
    fn deduplicate(&self, text: &str) -> String {
        if self.prev_tail_words.is_empty() {
            return text.to_owned();
        }
        // `text.split(/\s+/)`: on already-trimmed text this is the whitespace-run
        // split with no empty fragments.
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return text.to_owned();
        }
        let prev_tail = self.prev_tail_words.join(" ").to_lowercase();

        let mut best_cut = 0usize;
        let max_len = words.len().min(DEDUP_WINDOW);
        for len in 1..=max_len {
            let candidate = words[..len].join(" ").to_lowercase();
            // `endsWith(candidate) || includes(candidate)` — `includes` already
            // covers `endsWith`, but we keep both to mirror the JS exactly.
            if prev_tail.ends_with(&candidate) || prev_tail.contains(&candidate) {
                best_cut = len;
            }
        }
        if best_cut > 0 {
            words[best_cut..].join(" ")
        } else {
            text.to_owned()
        }
    }

    /// `words.slice(-DEDUP_WINDOW)` lower-cased — the new `prevTailWords`.
    ///
    /// The JS stores the raw-case words (`prevTailWords = words.slice(-N)`) and
    /// lower-cases them later when building `prevTail`. We store them lower-cased
    /// up front (the only use is the lower-cased join), which is behavior-identical
    /// and saves re-lowering each chunk.
    fn last_words_lower(text: &str, window: usize) -> Vec<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let start = words.len().saturating_sub(window);
        words[start..].iter().map(|w| w.to_lowercase()).collect()
    }

    /// Samples (at 16 kHz) → milliseconds, floored (matches the integer ms stamps
    /// used elsewhere).
    fn samples_to_ms(samples: u64) -> u64 {
        samples * 1000 / SAMPLE_RATE_HZ
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

    /// A tiny chunk size so chunk boundaries trip after a few samples — keeps the
    /// unit tests fast while exercising the exact same code paths.
    const TINY: WhisperStreamConfig = WhisperStreamConfig {
        chunk_samples: 4,
        vad_threshold: 0.0, // every non-empty chunk is "speech" unless we say otherwise
    };

    /// A loud chunk (RMS clearly above threshold).
    fn loud(n: usize) -> Vec<f32> {
        vec![0.5; n]
    }
    /// A silent chunk (RMS == 0).
    fn silent(n: usize) -> Vec<f32> {
        vec![0.0; n]
    }

    // ---- chunking ------------------------------------------------------------

    #[test]
    fn buffers_until_a_whole_chunk_then_emits_one_transcribe() {
        let mut p = WhisperStreamPolicy::new(TINY);
        // 3 of 4 samples: nothing yet.
        let (cmds, _) = p.push_samples(&loud(3));
        assert!(cmds.is_empty());
        assert_eq!(p.pending_samples(), 3);
        // One more: a whole chunk is ready.
        let (cmds, _) = p.push_samples(&loud(1));
        assert_eq!(
            cmds,
            vec![HostCommand::Transcribe {
                chunk: 0,
                start_ms: 0,
                end_ms: 0, // 4 samples @16k = 0 ms floored
            }]
        );
        assert_eq!(p.pending_samples(), 0);
    }

    #[test]
    fn leftover_samples_stay_buffered_no_overlap_no_drop() {
        let mut p = WhisperStreamPolicy::new(TINY);
        // 10 samples → 2 whole chunks (8 samples), 2 leftover.
        let (cmds, _) = p.push_samples(&loud(10));
        assert_eq!(cmds.len(), 2);
        assert_eq!(p.pending_samples(), 2);
        // Next 2 complete a 3rd chunk.
        let (cmds, _) = p.push_samples(&loud(2));
        assert_eq!(cmds.len(), 1);
        assert_eq!(p.pending_samples(), 0);
    }

    #[test]
    fn chunk_indices_and_ms_spans_advance_monotonically() {
        // Use a realistic 16k chunk so ms spans are meaningful.
        let cfg = WhisperStreamConfig {
            chunk_samples: 16_000,
            vad_threshold: 0.0,
        };
        let mut p = WhisperStreamPolicy::new(cfg);
        let (cmds, _) = p.push_samples(&loud(48_000)); // 3 chunks = 3 s
        assert_eq!(
            cmds,
            vec![
                HostCommand::Transcribe {
                    chunk: 0,
                    start_ms: 0,
                    end_ms: 1000
                },
                HostCommand::Transcribe {
                    chunk: 1,
                    start_ms: 1000,
                    end_ms: 2000
                },
                HostCommand::Transcribe {
                    chunk: 2,
                    start_ms: 2000,
                    end_ms: 3000
                },
            ]
        );
    }

    // ---- VAD gate ------------------------------------------------------------

    #[test]
    fn silent_chunk_is_skipped_not_transcribed() {
        let cfg = WhisperStreamConfig {
            chunk_samples: 4,
            vad_threshold: 0.008, // shipping threshold
        };
        let mut p = WhisperStreamPolicy::new(cfg);
        let (cmds, skips) = p.push_samples(&silent(4));
        assert!(cmds.is_empty());
        assert_eq!(skips, vec![SkipReason::Silence]);
        // The silent chunk was still CONSUMED (no re-buffering): a following loud
        // chunk is index 0 (the first one actually transcribed) and starts after
        // the skipped span.
        let (cmds, _) = p.push_samples(&loud(4));
        assert_eq!(
            cmds,
            vec![HostCommand::Transcribe {
                chunk: 0,
                start_ms: 0,
                end_ms: 0
            }]
        );
    }

    #[test]
    fn has_speech_matches_strided_rms_gate() {
        // RMS of a constant 0.5 signal is 0.5 > 0.008 → speech.
        assert!(WhisperStreamPolicy::has_speech(&[0.5; 16], 0.008));
        // RMS of silence is 0 → not speech.
        assert!(!WhisperStreamPolicy::has_speech(&[0.0; 16], 0.008));
        // Empty chunk → NaN in JS, false here.
        assert!(!WhisperStreamPolicy::has_speech(&[], 0.008));
        // A signal whose strided samples (every 4th) are all 0 reads as silence
        // even if intervening samples are loud — matches the JS stride.
        let mut a = vec![0.0f32; 16];
        for i in (1..16).step_by(4) {
            a[i] = 0.9; // loud, but never sampled by the step=4 stride (0,4,8,12)
        }
        assert!(!WhisperStreamPolicy::has_speech(&a, 0.008));
    }

    // ---- hallucination filter ------------------------------------------------

    #[test]
    fn known_hallucinations_are_dropped() {
        let mut p = WhisperStreamPolicy::new(TINY);
        // Decoded text passed straight to on_decoded (chunk feed not needed for the
        // text path).
        assert!(p.on_decoded("Thank you for watching").is_empty());
        assert!(p.on_decoded("thanks for watching.").is_empty()); // punctuation-stripped match
        assert!(p.on_decoded("The End").is_empty());
        assert!(p.on_decoded("Goodbye").is_empty());
    }

    #[test]
    fn lone_you_is_dropped_but_real_text_passes() {
        let mut p = WhisperStreamPolicy::new(TINY);
        assert!(p.on_decoded("you").is_empty());
        assert_eq!(
            p.on_decoded("you are here"),
            vec![TextEvent::Final("you are here".into())]
        );
    }

    #[test]
    fn repeated_single_word_4plus_is_a_hallucination() {
        assert!(WhisperStreamPolicy::is_hallucination("yeah yeah yeah yeah"));
        assert!(WhisperStreamPolicy::is_hallucination("no no no no no"));
        // 3 repeats is under the >= 4 threshold → not a hallucination.
        assert!(!WhisperStreamPolicy::is_hallucination("no no no"));
        // 4 DIFFERENT words → not a hallucination.
        assert!(!WhisperStreamPolicy::is_hallucination("the cat sat down"));
    }

    #[test]
    fn one_char_text_is_dropped() {
        let mut p = WhisperStreamPolicy::new(TINY);
        // `text.length > 1` guard.
        assert!(p.on_decoded("a").is_empty());
        assert!(p.on_decoded(" x ").is_empty()); // trims to "x", 1 char
        assert_eq!(p.on_decoded("ab"), vec![TextEvent::Final("ab".into())]);
    }

    // ---- tail dedup ----------------------------------------------------------

    #[test]
    fn first_chunk_has_no_dedup() {
        let mut p = WhisperStreamPolicy::new(TINY);
        assert_eq!(
            p.on_decoded("hello world this is the first chunk"),
            vec![TextEvent::Final(
                "hello world this is the first chunk".into()
            )]
        );
    }

    #[test]
    fn overlapping_leading_words_are_stripped_against_previous_tail() {
        let mut p = WhisperStreamPolicy::new(TINY);
        // Chunk 1 emits a tail ending in "the quick brown fox".
        assert_eq!(
            p.on_decoded("the quick brown fox"),
            vec![TextEvent::Final("the quick brown fox".into())]
        );
        // Chunk 2 re-emits "brown fox" then continues — dedup strips the longest
        // leading run found in the previous tail ("brown fox").
        assert_eq!(
            p.on_decoded("brown fox jumps over"),
            vec![TextEvent::Final("jumps over".into())]
        );
    }

    #[test]
    fn dedup_picks_the_longest_matching_leading_run() {
        let mut p = WhisperStreamPolicy::new(TINY);
        p.on_decoded("one two three four");
        // "one two three" is a prefix of the tail; the loop keeps the LARGEST len
        // that matches (3), cutting all three.
        assert_eq!(
            p.on_decoded("one two three five six"),
            vec![TextEvent::Final("five six".into())]
        );
    }

    #[test]
    fn dedup_that_consumes_whole_chunk_emits_nothing() {
        let mut p = WhisperStreamPolicy::new(TINY);
        p.on_decoded("alpha beta gamma");
        // Entire chunk is a duplicate of the tail → deduped to "" → no emit
        // (JS `if (text.trim().length > 0)`).
        assert!(p.on_decoded("beta gamma").is_empty());
        // Dedup state is NOT updated when nothing is emitted (JS only sets
        // prevTailWords inside the emit branch): the next chunk dedups against the
        // STILL-"alpha beta gamma" tail.
        assert_eq!(
            p.on_decoded("gamma delta"),
            vec![TextEvent::Final("delta".into())]
        );
    }

    // ---- stop / finalize -----------------------------------------------------

    #[test]
    fn stop_emits_one_finalize_then_none() {
        let mut p = WhisperStreamPolicy::new(TINY);
        p.push_samples(&loud(4));
        p.request_stop();
        assert_eq!(p.drain_finalize(), Some(HostCommand::Finalize));
        assert_eq!(p.drain_finalize(), None);
        assert_eq!(p.drain_finalize(), None);
    }

    #[test]
    fn no_finalize_before_stop() {
        let mut p = WhisperStreamPolicy::new(TINY);
        assert_eq!(p.drain_finalize(), None);
    }

    #[test]
    fn pushes_and_decodes_ignored_after_stop() {
        let mut p = WhisperStreamPolicy::new(TINY);
        p.request_stop();
        let (cmds, skips) = p.push_samples(&loud(100));
        assert!(cmds.is_empty() && skips.is_empty());
        assert!(p.on_decoded("ignored text here").is_empty());
    }

    #[test]
    fn reset_clears_buffer_and_dedup_and_stop() {
        let mut p = WhisperStreamPolicy::new(TINY);
        p.push_samples(&loud(2)); // partial buffer
        p.on_decoded("alpha beta gamma"); // sets dedup tail
        p.request_stop();
        p.reset();
        assert_eq!(p.pending_samples(), 0);
        // Fresh: no dedup against the old tail.
        assert_eq!(
            p.on_decoded("beta gamma delta"),
            vec![TextEvent::Final("beta gamma delta".into())]
        );
        // Stop was cleared too.
        assert_eq!(p.drain_finalize(), None);
    }

    // ---- config --------------------------------------------------------------

    #[test]
    fn shipping_configs_match_the_js_constants() {
        assert_eq!(WhisperStreamConfig::WHISPER_SOLO.chunk_samples, 16_000 * 5);
        assert_eq!(
            WhisperStreamConfig::MOONSHINE_DUAL.chunk_samples,
            16_000 * 3
        );
        assert!((WhisperStreamConfig::SHIPPING_VAD_THRESHOLD - 0.008).abs() < 1e-9);
    }

    // ---- serialization -------------------------------------------------------

    #[test]
    fn host_command_serializes_as_discriminated_union() {
        let t = HostCommand::Transcribe {
            chunk: 2,
            start_ms: 5000,
            end_ms: 10_000,
        };
        let j = serde_json::to_value(&t).unwrap();
        assert_eq!(j["cmd"], "transcribe");
        assert_eq!(j["chunk"], 2);
        assert_eq!(j["start_ms"], 5000);
        let back: HostCommand = serde_json::from_value(j).unwrap();
        assert_eq!(back, t);

        let f = HostCommand::Finalize;
        assert_eq!(serde_json::to_value(&f).unwrap()["cmd"], "finalize");
    }
}
