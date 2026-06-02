//! CLI: transcribe a 16 kHz mono WAV file with the Nemotron streaming engine.
//!
//! ```text
//! cargo run --release --example transcribe -- <wav_path> [model_dir]
//! ```
//!
//! `model_dir` defaults to `models`. Prints the transcript and the real-time
//! factor (compute time / audio duration).

use std::time::Instant;

use nemotron_asr::constants::SAMPLE_RATE;
use nemotron_asr::{audio, Nemotron};

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
