//! Shared mel-chunk construction and greedy-decode helpers.
//!
//! This module is the *one chunking core* that both the native
//! ([`crate::streaming`]) and wasm32 (`backend_web`, compiled only on
//! `target_arch = "wasm32"`) backends build on. Both paths implement the
//! **same** mel-frame layout policy:
//!
//! ```text
//! encoder input shape: [1, N_MELS, PRE_ENCODE_CACHE + CHUNK_SIZE]
//!                       ─────────  ──────────────────────────────
//!                        batch=1   lookback (9 frames) + main (≤56 frames)
//! ```
//!
//! The distinction between native and wasm is *only* in how the encoder and
//! decoder sessions are invoked (sync `ort` vs async `ort-web`); the policy
//! for which frames to include, how the lookback is aligned, and when to stop
//! consuming live-stream frames are all defined here and must not be
//! duplicated.
//!
//! ## Three mel-chunk variants
//!
//! | Builder | Used by | When |
//! |---------|---------|------|
//! | [`build_offline_mel_chunk`] | `streaming::transcribe_audio`, `backend_web::run_over_audio` | offline, full mel pre-computed |
//! | [`build_streaming_mel_chunk`] | `backend_web::stream_step` | live mic, audio buffered and mel recomputed |
//! | [`build_tail_mel_chunk`] | `backend_web::flush_tail` | end of stream, remaining frames < `CHUNK_SIZE` |
//!
//! The streaming variants do not have a native equivalent: the native path
//! always works on a complete audio buffer (no live-mic accumulation). Only
//! the offline builder is used on both sides; the streaming builders are
//! wasm32-only, but live here to keep all chunking policy in one auditable
//! place.

use ndarray::Array2;

use crate::constants::{CHUNK_SIZE, N_MELS, PRE_ENCODE_CACHE};

/// Build the flat mel-chunk buffer for the **offline** transcription path.
///
/// This implements the `[N_MELS, PRE_ENCODE_CACHE + CHUNK_SIZE]` layout
/// (flattened row-major, mel-major) used by both the native and wasm offline
/// paths. The layout mirrors `streaming::StreamingAsr::transcribe_audio` and
/// `backend_web::WasmAsr::run_over_audio`, which were previously independent
/// copies of the same algorithm.
///
/// # Arguments
///
/// - `mel` — full log-mel spectrogram `[N_MELS, total_frames]`.
/// - `chunk_idx` — zero-based chunk counter (0 = first chunk, no lookback).
/// - `buffer_idx` — start of the current main-chunk window in mel-frame units.
/// - `main_len` — number of main frames in this chunk (≤ `CHUNK_SIZE`; the
///   last chunk may be shorter).
///
/// # Returns
///
/// A `Vec<f32>` of length `N_MELS × (PRE_ENCODE_CACHE + CHUNK_SIZE)` in
/// mel-major row order, ready to be passed to the encoder as
/// `[1, N_MELS, PRE_ENCODE_CACHE + CHUNK_SIZE]`.
#[must_use]
pub fn build_offline_mel_chunk(
    mel: &Array2<f32>,
    chunk_idx: usize,
    buffer_idx: usize,
    main_len: usize,
) -> Vec<f32> {
    let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
    let mut chunk_data = vec![0.0f32; N_MELS * expected_size];

    // Pre-encode lookback: the up-to-9 frames immediately preceding this
    // chunk (only available from the second chunk onward).
    if chunk_idx > 0 && buffer_idx >= PRE_ENCODE_CACHE {
        let cache_start = buffer_idx - PRE_ENCODE_CACHE;
        for f in 0..PRE_ENCODE_CACHE {
            for m in 0..N_MELS {
                chunk_data[m * expected_size + f] = mel[[m, cache_start + f]];
            }
        }
    }

    // Main frames (≤ CHUNK_SIZE; the last chunk may be shorter).
    for f in 0..main_len {
        for m in 0..N_MELS {
            chunk_data[m * expected_size + PRE_ENCODE_CACHE + f] = mel[[m, buffer_idx + f]];
        }
    }

    chunk_data
}

/// Build the flat mel-chunk buffer for the **live-streaming** path.
///
/// Called by `backend_web::stream_step` for every full `CHUNK_SIZE`-frame
/// chunk consumed from the live audio buffer. Unlike the offline builder, the
/// mel is recomputed over the *whole retained audio buffer* on every call
/// (the "recompute over the whole buffer" fix) — so `main_start` may be
/// greater than or equal to `PRE_ENCODE_CACHE` even on later chunks, and the
/// lookback is right-aligned when fewer than `PRE_ENCODE_CACHE` prior frames
/// exist.
///
/// # Arguments
///
/// - `full_mel` — log-mel spectrogram of the retained audio buffer
///   `[N_MELS, total_frames]`.
/// - `chunk_idx` — zero-based streaming chunk counter.
/// - `main_start` — index of the first main-frame in `full_mel`
///   (`audio_processed / HOP_LENGTH`).
/// - `total_mel_frames` — `full_mel.shape()[1]` (passed in to avoid re-lookup).
///
/// # Returns
///
/// `(chunk_data, chunk_length)` where `chunk_data` is `N_MELS ×
/// (PRE_ENCODE_CACHE + CHUNK_SIZE)` mel-major and `chunk_length` is always
/// `PRE_ENCODE_CACHE + CHUNK_SIZE` (streaming chunks are always full).
#[must_use]
pub fn build_streaming_mel_chunk(
    full_mel: &Array2<f32>,
    chunk_idx: usize,
    main_start: usize,
    total_mel_frames: usize,
) -> (Vec<f32>, usize) {
    let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
    let mut chunk_data = vec![0.0f32; N_MELS * expected_size];

    if chunk_idx == 0 {
        // First chunk: no prior frames, so the 9 lookback slots stay zero;
        // fill the 56 main frames.
        for f in 0..CHUNK_SIZE.min(total_mel_frames) {
            for m in 0..N_MELS {
                chunk_data[m * expected_size + PRE_ENCODE_CACHE + f] = full_mel[[m, f]];
            }
        }
    } else {
        // Real pre-encode lookback: up-to-9 frames immediately before
        // `main_start`, right-aligned into the 9 lookback slots.
        let cache_start = main_start.saturating_sub(PRE_ENCODE_CACHE);
        let cache_frames = main_start - cache_start;
        let cache_offset = PRE_ENCODE_CACHE - cache_frames;
        for f in 0..cache_frames {
            for m in 0..N_MELS {
                chunk_data[m * expected_size + cache_offset + f] = full_mel[[m, cache_start + f]];
            }
        }
        // 56 main frames.
        for f in 0..CHUNK_SIZE.min(total_mel_frames - main_start) {
            for m in 0..N_MELS {
                chunk_data[m * expected_size + PRE_ENCODE_CACHE + f] =
                    full_mel[[m, main_start + f]];
            }
        }
    }

    // A streaming chunk always carries a full window; encoder length is the
    // full expected_size (matches the reference).
    (chunk_data, expected_size)
}

/// Build the flat mel-chunk buffer for the **tail-flush** path.
///
/// Called by `backend_web::flush_tail` to decode the `< CHUNK_SIZE` mel
/// frames left unconsumed after the final `stream_step`. Mirrors the last
/// iteration of the offline `transcribe_audio` where `main_len < CHUNK_SIZE`.
/// There is no edge-guard here: at end-of-stream the right-edge zero-padding
/// is legitimate.
///
/// # Arguments
///
/// - `full_mel` — log-mel spectrogram of the retained buffer
///   `[N_MELS, total_frames]`.
/// - `main_start` — index of the first unprocessed frame in `full_mel`.
/// - `available` — number of remaining frames (`total_frames - main_start`;
///   always < `CHUNK_SIZE` in practice).
///
/// # Returns
///
/// `(chunk_data, chunk_length)` where `chunk_data` is `N_MELS ×
/// (PRE_ENCODE_CACHE + CHUNK_SIZE)` mel-major (unused main slots are zero)
/// and `chunk_length` is `PRE_ENCODE_CACHE + available` (the partial-chunk
/// encoder length).
#[must_use]
pub fn build_tail_mel_chunk(
    full_mel: &Array2<f32>,
    main_start: usize,
    available: usize,
) -> (Vec<f32>, i64) {
    let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
    let mut chunk_data = vec![0.0f32; N_MELS * expected_size];

    // Lookback: up to 9 frames before `main_start`, right-aligned. When no
    // streaming chunk was ever consumed (`main_start == 0`, e.g. a sub-560ms
    // utterance), there is no prior audio and the lookback slots stay zero,
    // matching the offline first/only chunk.
    let cache_start = main_start.saturating_sub(PRE_ENCODE_CACHE);
    let cache_frames = main_start - cache_start;
    let cache_offset = PRE_ENCODE_CACHE - cache_frames;
    for f in 0..cache_frames {
        for m in 0..N_MELS {
            chunk_data[m * expected_size + cache_offset + f] = full_mel[[m, cache_start + f]];
        }
    }

    // Main frames (fewer than CHUNK_SIZE).
    for f in 0..available {
        for m in 0..N_MELS {
            chunk_data[m * expected_size + PRE_ENCODE_CACHE + f] = full_mel[[m, main_start + f]];
        }
    }

    // Partial-chunk encoder length: only PRE_ENCODE_CACHE + available frames
    // are valid; the encoder ignores frames beyond `length`.
    #[allow(clippy::cast_possible_wrap)] // PRE_ENCODE_CACHE + available ≤ 65, fits i64
    let chunk_length = (PRE_ENCODE_CACHE + available) as i64;

    (chunk_data, chunk_length)
}

/// Index of the maximum element (first on ties), matching `np.argmax`.
///
/// Used by both the native sync decode loop ([`crate::streaming`]) and the
/// wasm async decode loop (`backend_web`, compiled only on
/// `target_arch = "wasm32"`).
#[must_use]
pub fn argmax(values: &[f32]) -> usize {
    let mut max_idx = 0;
    let mut max_val = f32::NEG_INFINITY;
    for (i, &v) in values.iter().enumerate() {
        if v > max_val {
            max_val = v;
            max_idx = i;
        }
    }
    max_idx
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp, // Test values are exact powers-of-two (0.0, 1.0, 2.0, 3.0),
                           // representable exactly in f32; == comparison is correct here.
        clippy::cast_possible_wrap // PRE_ENCODE_CACHE + 10 = 19, safely within i64.
    )]
    use super::*;

    #[test]
    fn argmax_returns_first_max_on_ties() {
        assert_eq!(argmax(&[0.1, 0.5, 0.5, 0.2]), 1);
        assert_eq!(argmax(&[3.0, 1.0, 2.0]), 0);
    }

    #[test]
    fn build_offline_mel_chunk_first_chunk_has_zero_lookback() {
        use ndarray::Array2;
        let mel = Array2::from_elem((N_MELS, 56), 1.0f32);
        let chunk = build_offline_mel_chunk(&mel, 0, 0, 56);
        let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
        // First 9 frames (lookback) should be zero.
        for m in 0..N_MELS {
            for f in 0..PRE_ENCODE_CACHE {
                assert_eq!(chunk[m * expected_size + f], 0.0, "lookback[m={m},f={f}]");
            }
        }
        // Next 56 frames should be 1.0.
        for m in 0..N_MELS {
            for f in 0..CHUNK_SIZE {
                assert_eq!(
                    chunk[m * expected_size + PRE_ENCODE_CACHE + f],
                    1.0,
                    "main[m={m},f={f}]"
                );
            }
        }
    }

    #[test]
    fn build_offline_mel_chunk_second_chunk_fills_lookback() {
        use ndarray::Array2;
        // For the second chunk (chunk_idx=1, buffer_idx=CHUNK_SIZE=56),
        // the mel needs at least 2×CHUNK_SIZE frames to hold the main section.
        // We use 2×CHUNK_SIZE so the main indices [56..112) are in bounds.
        let total = 2 * CHUNK_SIZE;
        let mel = Array2::from_elem((N_MELS, total), 2.0f32);
        let chunk = build_offline_mel_chunk(&mel, 1, CHUNK_SIZE, CHUNK_SIZE);
        let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
        // Lookback (frames [CHUNK_SIZE - PRE_ENCODE_CACHE .. CHUNK_SIZE] from
        // `mel`) should all be 2.0 since the whole mel is filled with 2.0.
        for m in 0..N_MELS {
            for f in 0..PRE_ENCODE_CACHE {
                assert_eq!(chunk[m * expected_size + f], 2.0, "lookback[m={m},f={f}]");
            }
        }
    }

    #[test]
    fn build_tail_mel_chunk_zero_lookback_when_main_start_zero() {
        use ndarray::Array2;
        let mel = Array2::from_elem((N_MELS, 10), 3.0f32);
        let (chunk, length) = build_tail_mel_chunk(&mel, 0, 10);
        let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
        // No prior chunks → lookback is zero.
        for m in 0..N_MELS {
            for f in 0..PRE_ENCODE_CACHE {
                assert_eq!(chunk[m * expected_size + f], 0.0, "lookback[m={m},f={f}]");
            }
        }
        // 10 main frames filled.
        for m in 0..N_MELS {
            for f in 0..10 {
                assert_eq!(
                    chunk[m * expected_size + PRE_ENCODE_CACHE + f],
                    3.0,
                    "main[m={m},f={f}]"
                );
            }
        }
        assert_eq!(length, (PRE_ENCODE_CACHE + 10) as i64);
    }
}
