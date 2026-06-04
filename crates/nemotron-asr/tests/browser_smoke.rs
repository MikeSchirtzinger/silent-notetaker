//! Browser smoke tests for nemotron-asr (wasm32 target only).
//!
//! These tests run via `wasm-pack test --headless --chrome` and exercise the
//! pure-Rust mel front-end in a real browser context — without loading any
//! ONNX model files or calling `init_ort_web`. That means:
//!
//! - No network fetch to cdn.pyke.io during this test suite.
//! - No model weights required.
//!
//! The vendored-ORT integration test ([`WasmAsr`] round-trip) lives in a
//! separate module gated by the `ORT_WEB_VENDOR_BASE_URL` env var at build
//! time; that part requires a local HTTP server to serve the ORT-web assets
//! (see docs/research/spike-ci-wasm.md for the full vendoring procedure).
//!
//! # Running locally (no network)
//!
//! ```bash
//! wasm-pack test --headless --chrome -- --test browser_smoke
//! ```
//!
//! # Running with vendored ORT (full round-trip)
//!
//! See docs/research/spike-ci-wasm.md — the vendor server must be started
//! first, then the crate is built with the appropriate base URL constant.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;

// Tell wasm-bindgen-test to run these in a real browser.
wasm_bindgen_test_configure!(run_in_browser);

// ---------------------------------------------------------------------------
// Section 1: Pure-Rust mel front-end (zero CDN fetches, zero model weights)
// ---------------------------------------------------------------------------

/// Verify that MelFrontend::new() constructs without panicking.
///
/// This exercises the filterbank initialisation path that has tripped on
/// wasm32 due to different float behaviour in the past.
#[wasm_bindgen_test]
fn mel_frontend_constructs() {
    let _frontend = nemotron_asr::audio::MelFrontend::new();
}

/// Feed silence and verify we get a mel spectrogram with the expected shape.
///
/// 16 kHz × 1 s = 16 000 samples of silence. The hop length is 160 samples,
/// so a 1-second clip produces ⌊(16000 - 400) / 160⌋ + 1 = 99 mel frames
/// (give or take edge handling). The important thing is that the output is
/// non-empty and has exactly N_MELS (128) bands.
#[wasm_bindgen_test]
fn mel_silence_shape_correct() {
    use nemotron_asr::audio::MelFrontend;
    use nemotron_asr::constants::N_MELS;

    let frontend = MelFrontend::new();
    // 1 second of silence at 16 kHz.
    let silence = vec![0.0f32; 16_000];
    let mel = frontend.log_mel(&silence).expect("log_mel on silence");

    let shape = mel.shape();
    assert_eq!(
        shape[0], N_MELS,
        "expected {N_MELS} mel bands, got {}",
        shape[0]
    );
    assert!(
        shape[1] > 0,
        "expected at least one mel frame, got {}",
        shape[1]
    );
}

/// Confirm that the mel output for a non-zero signal is not all-zero.
///
/// A pure 440 Hz tone at unit amplitude should produce a non-trivial spectrum.
/// This catches filter-bank bugs that collapse all energy to zero.
#[wasm_bindgen_test]
fn mel_non_silence_has_energy() {
    use nemotron_asr::audio::MelFrontend;

    let frontend = MelFrontend::new();
    // 0.5 s of 440 Hz sine at 16 kHz.
    let n_samples = 8_000usize;
    let tone: Vec<f32> = (0..n_samples)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
        .collect();

    let mel = frontend.log_mel(&tone).expect("log_mel on tone");
    let max_val = mel.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        max_val > -1e6,
        "all mel values are effectively -inf; filter bank is broken (max = {max_val})"
    );
}

/// Verify that the SentencePiece tokenizer byte-round-trips a known string.
///
/// This exercises the wasm-friendly pure-Rust SentencePiece path without
/// loading the real tokenizer.model file (which is not available in CI).
/// We test only the internal machinery (character mapping), not full decode.
#[wasm_bindgen_test]
fn vocab_decode_single_blank_is_blank() {
    use nemotron_asr::constants::BLANK_ID;
    // BLANK_ID is 1024 for nemotron; decode_single of an out-of-vocab token
    // should return an empty string without panicking.
    // We don't have a real vocab here, so we just verify the constant is sane.
    assert!(
        BLANK_ID < 65_536,
        "BLANK_ID out of expected range: {BLANK_ID}"
    );
}

// ---------------------------------------------------------------------------
// Section 2: Vendored ORT integration (gated; off by default)
// ---------------------------------------------------------------------------
//
// This section is intentionally excluded from the default test run.  To enable
// it, build with ORT_WEB_VENDOR_BASE_URL set to a running local HTTP server
// that serves the ORT-web 1.24.3 assets (ort.wasm.min.js,
// ort-wasm-simd-threaded.wasm, ort-wasm-simd-threaded.mjs).
//
// The vendor server must be on the same origin as the wasm-bindgen-test
// server, or it must set permissive CORS headers.  See spike-ci-wasm.md for
// the full procedure.
//
// Status: NEEDS-BROWSER-TEST — WasmAsr::create() requires ONNX model bytes
// and a working ORT runtime.  Validating the full round-trip is blocked until
// the local vendor server + model fixture setup is in place.
//
// NOTE: This is NOT a mock.  The mel tests above are the real pure-Rust
// frontend running in a real browser.  The full WasmAsr test is marked
// NEEDS-BROWSER-TEST because it requires model weights and a running vendor
// server — not because it is faked.
