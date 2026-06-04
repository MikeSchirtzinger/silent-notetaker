//! CLI: transcribe a 16 kHz mono WAV file with the Nemotron streaming engine.
//!
//! ```text
//! cargo run --release --example transcribe -- <wav_path> [model_dir]
//! ```
//!
//! `model_dir` defaults to `models`. Prints the transcript and the real-time
//! factor (compute time / audio duration).
//!
//! This is a **native-only** CLI: it links `Nemotron` (the native `OrtBackend`
//! alias, cfg-gated off wasm32 in `lib.rs`) and reads WAV files from the
//! filesystem. It is gated off `wasm32` so `cargo test --target
//! wasm32-unknown-unknown` (which `wasm-pack test` runs, compiling examples
//! too) does not try to build it against the absent native symbols.

// On wasm32 the example is an empty stub: nothing to run in a browser.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use nemotron_asr::constants::SAMPLE_RATE;
#[cfg(not(target_arch = "wasm32"))]
use nemotron_asr::{audio, Nemotron};

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let wav_path = args.next().unwrap_or_else(|| {
        eprintln!("usage: transcribe <wav_path> [model_dir]");
        std::process::exit(2);
    });
    let model_dir = args.next().unwrap_or_else(|| "models".to_string());

    eprintln!("loading model from {model_dir} ...");
    let load_start = Instant::now();
    let mut asr = Nemotron::from_pretrained(&model_dir)?;
    eprintln!("model loaded in {:.2}s", load_start.elapsed().as_secs_f64());

    let samples = audio::load_wav_mono(&wav_path)?;
    // sample count and sample rate fit well within f64 mantissa for typical audio files.
    #[allow(clippy::cast_precision_loss)]
    let audio_secs = samples.len() as f64 / SAMPLE_RATE as f64;
    eprintln!("audio: {audio_secs:.2}s ({} samples)", samples.len());

    let start = Instant::now();
    let transcript = asr.transcribe_audio(&samples)?;
    let compute = start.elapsed().as_secs_f64();
    let rtf = compute / audio_secs;

    println!("\n=== TRANSCRIPT ===\n{transcript}");
    println!("\ncompute = {compute:.2}s for {audio_secs:.2}s audio  =>  RTF = {rtf:.3}x");

    Ok(())
}
