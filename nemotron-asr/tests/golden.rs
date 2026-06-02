//! Validation gate: transcribe `test-assets/test_16k.wav` and assert the
//! output matches `test-assets/golden_transcript.txt`.
//!
//! Comparison is case-insensitive and whitespace/punctuation-normalised on a
//! per-word basis. The words must all be correct and in order.
//!
//! Requires the model files under `models/` (gitignored). The test is skipped
//! with a clear message if they are absent, so `cargo test` stays green in
//! environments without the ~900 MB weights.

use std::path::Path;

use nemotron_asr::{audio, Nemotron};

/// Lowercase, strip everything that is not a letter, digit, or space, and
/// split into words.
fn normalize_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn transcribes_golden_clip() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let model_dir = Path::new(crate_dir).join("models");
    let wav = Path::new(crate_dir).join("test-assets/test_16k.wav");
    let golden = Path::new(crate_dir).join("test-assets/golden_transcript.txt");

    if !model_dir.join("encoder.onnx").exists() {
        eprintln!(
            "skipping: model weights not found at {} — download them to run the validation gate",
            model_dir.display()
        );
        return;
    }

    let expected = std::fs::read_to_string(&golden).expect("read golden transcript");
    let expected_words = normalize_words(&expected);

    let mut asr = Nemotron::from_pretrained(&model_dir).expect("load model");
    let samples = audio::load_wav_mono(&wav).expect("load wav");
    let transcript = asr.transcribe_audio(&samples).expect("transcribe");

    eprintln!("transcript: {transcript:?}");
    let got_words = normalize_words(&transcript);

    assert_eq!(
        got_words, expected_words,
        "\n  expected words: {expected_words:?}\n  got words:      {got_words:?}"
    );
}
