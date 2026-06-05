//! SenseVoice solo VAD-segmentation policy, as a Rust policy module.
//!
//! Ports the *segmentation decisions* of `index.html`'s `SenseVoiceEngine`
//! (`processAudio` + the VAD/recognizer config) into deterministic, browser-free
//! Rust policy (PRD R2, Phase 5; Appendix A row 11: *"SenseVoice solo (30 s
//! window segmentation)"*). The actual VAD inference and ASR decode run in the
//! **js-sherpa** host (the sherpa-onnx Emscripten harness); this module owns the
//! policy around them: how audio is buffered and windowed into the VAD, the VAD
//! parameters, and the per-segment **> 0.3 s decode gate** that decides whether a
//! detected speech segment is worth decoding at all.
//!
//! # What the JS did, and what is policy
//!
//! `SenseVoiceEngine.processAudio(samples)` (`index.html`):
//!
//! ```text
//! buffer.push(samples);                                  // 30 s CircularBuffer
//! const windowSize = vad.config.sileroVad.windowSize;    // 512
//! while (buffer.size() > windowSize) {
//!   const w = buffer.get(buffer.head(), windowSize);
//!   vad.acceptWaveform(w);                               // feed VAD a 512 window
//!   buffer.pop(windowSize);
//! }
//! while (!vad.isEmpty()) {
//!   const seg = vad.front();
//!   const durationSec = seg.samples.length / 16000;
//!   if (durationSec > 0.3) { decode → emit onResult(text, seg.samples); }
//!   vad.pop();
//! }
//! ```
//!
//! The **decisions** — the 30 s circular-buffer capacity, the 512-sample VAD
//! window draining, the Silero VAD parameters (threshold, min-speech,
//! min-silence, the **30 s max-speech window** that is the headline of this row),
//! and the `durationSec > 0.3` decode gate — are pure policy and live here,
//! tested without a browser or the sherpa runtime. The VAD model and the ASR
//! decode are the host's job; the policy tells the host *which windows to feed the
//! VAD* and *which detected segments to decode*.
//!
//! # Modeling the host VAD without running it
//!
//! The real Silero VAD is a neural model; we do not reimplement it. The policy is
//! split so it is testable:
//!
//! - [`SenseVoicePolicy::push_samples`] performs the **buffering + windowing**
//!   decision: it drains the buffer into `windowSize` windows and emits one
//!   [`HostCommand::FeedVadWindow`] per window for the host to run the VAD on, and
//!   keeps the < `windowSize` leftover buffered. This is exact, deterministic, and
//!   tested directly.
//! - [`SenseVoicePolicy::on_vad_segment`] applies the **decode gate**: the host
//!   reports a VAD-detected segment's sample length; the policy returns whether to
//!   decode it ([`SegmentDecision`]) per the `> 0.3 s` rule and the 30 s window
//!   cap, so the *gate* is unit-tested against the exact JS threshold.
//!
//! No `unwrap`/`expect`; no fallible op on the hot path (PRD "Rust engineering bar").

use serde::{Deserialize, Serialize};

/// 16 kHz mono, as `usize` for the buffer-capacity sample count.
const SAMPLE_RATE_HZ_USIZE: usize = 16_000;

/// 16 kHz mono, as `f32` for the seconds↔samples gate division (matches the JS
/// `length / 16000` float math exactly).
const SAMPLE_RATE_HZ_F32: f32 = 16_000.0;

/// The Silero VAD parameters the app pins, plus the buffering/gate constants —
/// the exact values from `SenseVoiceEngine._initVAD()` / `_initRecognizer()`.
///
/// These are policy *data*: in the running app they become registry config
/// (PRD R4 / Task I3). Pinned here so the policy matches shipping and tests can
/// shrink them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SenseVoiceConfig {
    /// Silero VAD speech probability threshold (JS `threshold: 0.5`).
    pub vad_threshold: f32,
    /// Minimum speech duration to start a segment, seconds (JS `minSpeechDuration: 0.25`).
    pub min_speech_secs: f32,
    /// Minimum silence to end a segment, seconds (JS `minSilenceDuration: 0.5`).
    pub min_silence_secs: f32,
    /// **The 30 s window**: maximum speech duration before the VAD force-cuts a
    /// segment, seconds (JS `maxSpeechDuration: 30`). This is the "30 s window
    /// segmentation" of Appendix A row 11 — SenseVoice is offline, so without this
    /// cap a long monologue would be one unbounded segment.
    pub max_speech_secs: f32,
    /// VAD window size in samples (JS `windowSize: 512`): the granule the buffer
    /// is drained into the VAD.
    pub window_samples: usize,
    /// Circular-buffer capacity in samples (JS `new CircularBuffer(30 * 16000)`):
    /// 30 s of audio held between `processAudio` calls.
    pub buffer_capacity_samples: usize,
    /// Per-segment decode gate, seconds: a detected segment is decoded only if
    /// strictly longer than this (JS `if (durationSec > 0.3)`).
    pub min_decode_secs: f32,
}

impl SenseVoiceConfig {
    /// The exact shipping SenseVoice values.
    pub const SHIPPING: SenseVoiceConfig = SenseVoiceConfig {
        vad_threshold: 0.5,
        min_speech_secs: 0.25,
        min_silence_secs: 0.5,
        max_speech_secs: 30.0,
        window_samples: 512,
        buffer_capacity_samples: 30 * SAMPLE_RATE_HZ_USIZE,
        min_decode_secs: 0.3,
    };
}

impl Default for SenseVoiceConfig {
    fn default() -> Self {
        Self::SHIPPING
    }
}

/// A typed command the policy emits for the js-sherpa host to execute.
///
/// `#[serde(tag = "cmd")]` discriminated union; `#[non_exhaustive]` for additive
/// commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostCommand {
    /// Feed exactly one `window_samples`-long window to the VAD (`vad.acceptWaveform`).
    /// The host runs the VAD model on it and later reports any completed segments
    /// via [`SenseVoicePolicy::on_vad_segment`]. Carries the window's absolute
    /// sample offset so segment spans can be stamped without a shared clock.
    FeedVadWindow {
        /// Absolute sample offset of this window's first sample, from session start.
        offset_samples: u64,
    },
    /// Decode a VAD-detected segment and emit its text (the host's
    /// `recognizer.createStream()` → `acceptWaveform` → `decode` → `getResult`).
    /// Emitted by the policy only for segments that pass the decode gate.
    DecodeSegment {
        /// Monotonic segment index (the JS `vad.front()` FIFO order made explicit).
        segment: u32,
        /// Absolute sample offset of the segment's first sample, from session start.
        offset_samples: u64,
        /// Length of the segment in samples (the host owns the samples; this is the
        /// span the policy gated on).
        length_samples: u64,
    },
    /// End of stream: tear down the recognizer/VAD (JS `destroy()`).
    Finalize,
}

/// Whether a VAD-detected segment should be decoded.
///
/// `#[non_exhaustive]` so a future skip reason (e.g. an explicit
/// over-max-window classification) is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SegmentDecision {
    /// The segment passed the `> 0.3 s` gate — decode it.
    Decode,
    /// The segment was at or below the decode gate — drop it (JS: the `if`
    /// guard is false, the segment is `vad.pop()`-ed without decoding).
    TooShort,
}

/// The SenseVoice solo VAD-segmentation policy.
///
/// Push captured 16 kHz samples ([`push_samples`]) to get the VAD-window feed
/// commands; report each VAD-detected segment's length ([`on_vad_segment`]) to
/// get the gate decision (and, when it passes, a [`HostCommand::DecodeSegment`]).
/// The policy owns the buffering, windowing, the 30 s window cap (as config the
/// host's VAD enforces), and the decode gate; the host runs the VAD + ASR.
///
/// [`push_samples`]: SenseVoicePolicy::push_samples
/// [`on_vad_segment`]: SenseVoicePolicy::on_vad_segment
#[derive(Debug, Clone)]
pub struct SenseVoicePolicy {
    config: SenseVoiceConfig,
    /// The circular buffer's logical contents (JS `CircularBuffer`). We model it as
    /// a count of buffered samples + the absolute offset of the buffer head; the
    /// host owns the actual samples it will feed the VAD. Only the *windowing
    /// arithmetic* is policy.
    buffered: usize,
    /// Absolute sample offset of the next window to feed (advances by
    /// `window_samples` per drained window). JS `buffer.head()` made absolute.
    head_offset: u64,
    /// Monotonic segment index for [`HostCommand::DecodeSegment`].
    next_segment: u32,
    /// Latched on stop.
    stopped: bool,
    /// Whether [`HostCommand::Finalize`] was already emitted (idempotency).
    finalized: bool,
}

impl SenseVoicePolicy {
    /// Build a policy with the given config. Use [`SenseVoiceConfig::SHIPPING`] for
    /// the exact production behavior.
    #[must_use]
    pub fn new(config: SenseVoiceConfig) -> Self {
        Self {
            config,
            buffered: 0,
            head_offset: 0,
            next_segment: 0,
            stopped: false,
            finalized: false,
        }
    }

    /// The config in force (including the VAD parameters the host applies).
    #[must_use]
    pub fn config(&self) -> SenseVoiceConfig {
        self.config
    }

    /// Samples currently buffered awaiting a whole VAD window (`< window_samples`).
    #[must_use]
    pub fn buffered_samples(&self) -> usize {
        self.buffered
    }

    /// Push captured 16 kHz samples and emit one [`HostCommand::FeedVadWindow`] per
    /// whole `window_samples` window now drainable.
    ///
    /// Byte-for-byte port of the windowing loop:
    /// ```text
    /// buffer.push(samples);
    /// while (buffer.size() > windowSize) { feed one window; buffer.pop(windowSize); }
    /// ```
    /// **Note the JS `>` (strict):** a window is only drained while *more than*
    /// `windowSize` samples are buffered, so exactly `windowSize` samples are held
    /// back, not drained — reproduced here exactly (a subtle off-by-one that a
    /// naive `>=` would get wrong).
    pub fn push_samples(&mut self, sample_count: usize) -> Vec<HostCommand> {
        let mut commands = Vec::new();
        if self.stopped {
            return commands;
        }
        self.buffered += sample_count;
        let window = self.config.window_samples;
        // `while (buffer.size() > windowSize)`.
        while self.buffered > window {
            commands.push(HostCommand::FeedVadWindow {
                offset_samples: self.head_offset,
            });
            self.buffered -= window;
            self.head_offset += window as u64;
        }
        commands
    }

    /// Report a VAD-detected segment (the host's `vad.front()`), returning the gate
    /// decision and, when it passes, the [`HostCommand::DecodeSegment`] to issue.
    ///
    /// The gate is the exact JS `durationSec > 0.3` with `durationSec =
    /// length_samples / 16000` — reproduced as the same float division + compare so
    /// the boundary case (`length_samples == 4800` → `0.3 > 0.3` is `false`) lands
    /// identically.
    ///
    /// `offset_samples` is the segment's absolute start (the host reports it from
    /// the windows it fed). A segment that fails the gate yields
    /// [`SegmentDecision::TooShort`] and no command (the JS `vad.pop()`s it without
    /// decoding).
    pub fn on_vad_segment(
        &mut self,
        offset_samples: u64,
        length_samples: u64,
    ) -> (SegmentDecision, Option<HostCommand>) {
        if self.stopped {
            return (SegmentDecision::TooShort, None);
        }
        // `const durationSec = segment.samples.length / 16000; if (durationSec > 0.3)`.
        #[allow(
            clippy::cast_precision_loss,
            reason = "segment lengths are small sample counts (a 30 s max-window \
                      segment is 480_000 samples, far under 2^24 where f32 stays \
                      exact for integers); the f32 division reproduces the JS float \
                      gate byte-for-byte. Not on a tight inner loop — one call per \
                      detected segment."
        )]
        let duration_sec = length_samples as f32 / SAMPLE_RATE_HZ_F32;
        if duration_sec > self.config.min_decode_secs {
            let segment = self.next_segment;
            self.next_segment += 1;
            (
                SegmentDecision::Decode,
                Some(HostCommand::DecodeSegment {
                    segment,
                    offset_samples,
                    length_samples,
                }),
            )
        } else {
            (SegmentDecision::TooShort, None)
        }
    }

    /// Request stop (JS `destroy()`). The next [`drain_finalize`] emits the single
    /// [`HostCommand::Finalize`]; afterwards pushes/segments are no-ops. Safe to
    /// call repeatedly.
    ///
    /// [`drain_finalize`]: SenseVoicePolicy::drain_finalize
    pub fn request_stop(&mut self) {
        self.stopped = true;
    }

    /// Emit the one-shot [`HostCommand::Finalize`] after stop (idempotent). The JS
    /// `destroy()` drops any buffered audio and in-flight VAD segments with no
    /// trailing flush, so finalize is teardown only. Returns `None` before stop or
    /// after finalize.
    pub fn drain_finalize(&mut self) -> Option<HostCommand> {
        if !self.stopped || self.finalized {
            return None;
        }
        self.finalized = true;
        Some(HostCommand::Finalize)
    }

    /// Clear all streaming state for a fresh session. Keeps the config.
    pub fn reset(&mut self) {
        self.buffered = 0;
        self.head_offset = 0;
        self.next_segment = 0;
        self.stopped = false;
        self.finalized = false;
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

    /// A tiny window so the drain loop trips after a few samples.
    const TINY: SenseVoiceConfig = SenseVoiceConfig {
        window_samples: 4,
        ..SenseVoiceConfig::SHIPPING
    };

    // ---- windowing -----------------------------------------------------------

    #[test]
    fn drains_strictly_more_than_a_window_holding_one_window_back() {
        // JS `while (buffer.size() > windowSize)`: with exactly windowSize buffered,
        // NOTHING drains (strict >).
        let mut p = SenseVoicePolicy::new(TINY);
        let cmds = p.push_samples(4); // == window
        assert!(
            cmds.is_empty(),
            "exactly one window must NOT drain (strict >)"
        );
        assert_eq!(p.buffered_samples(), 4);
        // One more sample → now 5 > 4, one window drains, one sample remains.
        let cmds = p.push_samples(1);
        assert_eq!(cmds, vec![HostCommand::FeedVadWindow { offset_samples: 0 }]);
        assert_eq!(p.buffered_samples(), 1);
    }

    #[test]
    fn multiple_windows_drain_with_advancing_offsets() {
        let mut p = SenseVoicePolicy::new(TINY);
        // 13 samples → buffered 13 > 4 drains windows at 0,4,8 (leaving 1, since
        // after draining 3 windows buffered = 1, which is not > 4). Offsets advance
        // by window each.
        let cmds = p.push_samples(13);
        assert_eq!(
            cmds,
            vec![
                HostCommand::FeedVadWindow { offset_samples: 0 },
                HostCommand::FeedVadWindow { offset_samples: 4 },
                HostCommand::FeedVadWindow { offset_samples: 8 },
            ]
        );
        assert_eq!(p.buffered_samples(), 1);
    }

    #[test]
    fn windowing_is_continuous_across_pushes() {
        let mut p = SenseVoicePolicy::new(TINY);
        p.push_samples(3); // buffered 3, no drain
        let cmds = p.push_samples(3); // buffered 6 > 4 → one window at 0, leftover 2
        assert_eq!(cmds, vec![HostCommand::FeedVadWindow { offset_samples: 0 }]);
        assert_eq!(p.buffered_samples(), 2);
        let cmds = p.push_samples(3); // buffered 5 > 4 → window at 4, leftover 1
        assert_eq!(cmds, vec![HostCommand::FeedVadWindow { offset_samples: 4 }]);
        assert_eq!(p.buffered_samples(), 1);
    }

    // ---- decode gate (> 0.3 s) ----------------------------------------------

    #[test]
    fn segment_longer_than_decode_gate_is_decoded() {
        let mut p = SenseVoicePolicy::new(SenseVoiceConfig::SHIPPING);
        // 0.5 s = 8000 samples > 0.3 s → decode.
        let (decision, cmd) = p.on_vad_segment(1000, 8000);
        assert_eq!(decision, SegmentDecision::Decode);
        assert_eq!(
            cmd,
            Some(HostCommand::DecodeSegment {
                segment: 0,
                offset_samples: 1000,
                length_samples: 8000,
            })
        );
    }

    #[test]
    fn segment_at_or_below_decode_gate_is_dropped() {
        let mut p = SenseVoicePolicy::new(SenseVoiceConfig::SHIPPING);
        // 0.3 s exactly = 4800 samples → `0.3 > 0.3` is FALSE → TooShort.
        let (decision, cmd) = p.on_vad_segment(0, 4800);
        assert_eq!(decision, SegmentDecision::TooShort);
        assert!(cmd.is_none());
        // 0.2 s = 3200 samples → too short.
        let (decision, _) = p.on_vad_segment(0, 3200);
        assert_eq!(decision, SegmentDecision::TooShort);
        // Just over: 4801 samples = 0.3000625 s > 0.3 → decode.
        let (decision, _) = p.on_vad_segment(0, 4801);
        assert_eq!(decision, SegmentDecision::Decode);
    }

    #[test]
    fn segment_indices_advance_only_for_decoded_segments() {
        let mut p = SenseVoicePolicy::new(SenseVoiceConfig::SHIPPING);
        // decode → segment 0
        let (_, cmd) = p.on_vad_segment(0, 8000);
        assert!(matches!(
            cmd,
            Some(HostCommand::DecodeSegment { segment: 0, .. })
        ));
        // too short → no index consumed
        p.on_vad_segment(8000, 1000);
        // next decode → segment 1 (not 2)
        let (_, cmd) = p.on_vad_segment(9000, 8000);
        assert!(matches!(
            cmd,
            Some(HostCommand::DecodeSegment { segment: 1, .. })
        ));
    }

    // ---- stop / finalize -----------------------------------------------------

    #[test]
    fn stop_emits_one_finalize_then_none() {
        let mut p = SenseVoicePolicy::new(TINY);
        p.push_samples(100);
        p.request_stop();
        assert_eq!(p.drain_finalize(), Some(HostCommand::Finalize));
        assert_eq!(p.drain_finalize(), None);
    }

    #[test]
    fn pushes_and_segments_ignored_after_stop() {
        let mut p = SenseVoicePolicy::new(TINY);
        p.request_stop();
        assert!(p.push_samples(1000).is_empty());
        let (decision, cmd) = p.on_vad_segment(0, 99_999);
        assert_eq!(decision, SegmentDecision::TooShort);
        assert!(cmd.is_none());
    }

    #[test]
    fn reset_clears_state() {
        let mut p = SenseVoicePolicy::new(TINY);
        p.push_samples(3);
        p.on_vad_segment(0, 8000);
        p.request_stop();
        p.reset();
        assert_eq!(p.buffered_samples(), 0);
        // Fresh segment index starts at 0 again.
        let (_, cmd) = p.on_vad_segment(0, 8000);
        assert!(matches!(
            cmd,
            Some(HostCommand::DecodeSegment { segment: 0, .. })
        ));
        assert_eq!(p.drain_finalize(), None);
    }

    // ---- config --------------------------------------------------------------

    #[test]
    fn shipping_config_matches_js_constants() {
        let c = SenseVoiceConfig::SHIPPING;
        assert!((c.vad_threshold - 0.5).abs() < 1e-9);
        assert!((c.min_speech_secs - 0.25).abs() < 1e-9);
        assert!((c.min_silence_secs - 0.5).abs() < 1e-9);
        assert!((c.max_speech_secs - 30.0).abs() < 1e-9); // the "30 s window"
        assert_eq!(c.window_samples, 512);
        assert_eq!(c.buffer_capacity_samples, 30 * 16_000);
        assert!((c.min_decode_secs - 0.3).abs() < 1e-9);
    }

    #[test]
    fn host_command_serializes_as_discriminated_union() {
        let d = HostCommand::DecodeSegment {
            segment: 3,
            offset_samples: 16_000,
            length_samples: 8000,
        };
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["cmd"], "decode_segment");
        assert_eq!(j["segment"], 3);
        let back: HostCommand = serde_json::from_value(j).unwrap();
        assert_eq!(back, d);

        let w = HostCommand::FeedVadWindow {
            offset_samples: 512,
        };
        assert_eq!(serde_json::to_value(&w).unwrap()["cmd"], "feed_vad_window");
    }
}
