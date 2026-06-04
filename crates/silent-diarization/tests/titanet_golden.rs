//! Native TitaNet embedder golden test (cosine 1.000000 vs the JS reference).
//!
//! Productionization gate for spike b1: the [`TitaNetEmbedder`] must reproduce
//! the reference embeddings in `eval/js/ref/*.json` at cosine ~1.0 (the spike
//! measured exactly 1.000000 native), and the mel features at maxAbsDiff < 1e-3.
//!
//! # Weights are not committed (skips LOUDLY)
//!
//! E1 removed `titanet.onnx` + `mel_fb.json` from the repo. This test fetches
//! NOTHING on its own — it reads the weights from env vars and SKIPS WITH A LOUD
//! WARNING if they are absent, so a green test suite without the weights can
//! never be mistaken for a passing embedder gate (SESSION STANDARDS: skip paths
//! must be LOUD; no mock satisfies acceptance).
//!
//! Run it for real:
//! ```text
//! # download the registry-pinned weights (sha256 ad4a1802…789e):
//! curl -L -o /tmp/titanet.onnx \
//!   https://huggingface.co/FluffyBunnies/titanet-small-onnx/resolve/5fae6d4e517a019cab845fd98935fd5b3776dfed/titanet.onnx
//! SILENT_TITANET_ONNX=/tmp/titanet.onnx \
//! SILENT_MEL_FB=$PWD/eval/js/mel_fb.json \
//! SILENT_TITANET_FIXTURES=$PWD/eval/js/ref \
//!   cargo test -p silent-diarization --test titanet_golden -- --nocapture
//! ```
#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism (PRD lint config)"
)]

use serde::Deserialize;
use silent_audio::MelFrontend;
use silent_diarization::embedder::{TitaNetEmbedder, cosine_similarity};
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct RefFixture {
    samples: Vec<f32>,
    feat: Vec<Vec<f64>>,
    emb: Vec<f32>,
}

/// Returns `Some((onnx, mel_fb, fixtures_dir))` if all weight env vars are set,
/// else `None` (caller skips loudly).
fn weights() -> Option<(String, String, PathBuf)> {
    let onnx = std::env::var("SILENT_TITANET_ONNX").ok()?;
    let mel_fb = std::env::var("SILENT_MEL_FB").ok()?;
    let fixtures = std::env::var("SILENT_TITANET_FIXTURES").map_or_else(
        |_| {
            // Default: repo eval/js/ref relative to the crate.
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("eval")
                .join("js")
                .join("ref")
        },
        PathBuf::from,
    );
    Some((onnx, mel_fb, fixtures))
}

#[test]
fn titanet_cosine_golden() {
    let Some((onnx, mel_fb, fixtures_dir)) = weights() else {
        eprintln!(
            "\n================ TITANET GOLDEN SKIPPED (LOUD) ================\n\
             The TitaNet weights are NOT committed (E1 removed them). This test\n\
             did NOT run the embedder gate. To run it, set:\n\
               SILENT_TITANET_ONNX=<path to titanet.onnx>   (registry sha256 ad4a1802…789e)\n\
               SILENT_MEL_FB=<path to mel_fb.json>          (e.g. eval/js/mel_fb.json)\n\
               SILENT_TITANET_FIXTURES=<path to eval/js/ref>  (optional; defaults to repo eval/js/ref)\n\
             See the module docs for the exact download command.\n\
             ==============================================================\n"
        );
        return;
    };

    let mut embedder = TitaNetEmbedder::from_files(&onnx, &mel_fb)
        .expect("build TitaNetEmbedder from the registry-pinned weights");

    // A separate frontend for the feature-level comparison.
    let mel_fb_bytes = std::fs::read(&mel_fb).expect("read mel_fb.json");
    let frontend =
        MelFrontend::titanet_from_mel_fb_json(&mel_fb_bytes).expect("build TitaNet frontend");

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| panic!("read fixtures dir {}: {e}", fixtures_dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "no *.json fixtures in {}",
        fixtures_dir.display()
    );

    let mut worst_cos = 1.0_f32;
    let mut worst_feat = 0.0_f64;

    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(path).unwrap();
        let fx: RefFixture =
            serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"));

        // Feature-level comparison.
        let mel = frontend.log_mel(&fx.samples).unwrap();
        let t = mel.shape()[1].min(fx.feat[0].len());
        let mut max_abs = 0.0_f64;
        for m in 0..fx.feat.len() {
            for frame in 0..t {
                let diff = (f64::from(mel[[m, frame]]) - fx.feat[m][frame]).abs();
                max_abs = max_abs.max(diff);
            }
        }

        // Embedding-level comparison.
        let rust_emb = embedder.embed(&fx.samples).unwrap();
        let cos = cosine_similarity(&rust_emb, &fx.emb);

        worst_cos = worst_cos.min(cos);
        worst_feat = worst_feat.max(max_abs);

        println!("{name:<36} feat maxAbsDiff={max_abs:.2e}  embCos={cos:.6}");

        assert!(
            max_abs < 1e-3,
            "{name}: feat maxAbsDiff {max_abs:.2e} >= 1e-3"
        );
        assert!(cos >= 0.9999, "{name}: cosine {cos:.6} < 0.9999");
    }

    println!("WORST feat maxAbsDiff={worst_feat:.2e}  WORST embedding cosine={worst_cos:.6}");
    // The spike measured exactly 1.000000 native; require it productionized.
    assert!(
        worst_cos >= 0.999_999,
        "productionized embedder must hold cosine ~1.0 (worst {worst_cos:.6}); \
         a regression means the frontend or the ort wiring drifted from spike b1"
    );
}
