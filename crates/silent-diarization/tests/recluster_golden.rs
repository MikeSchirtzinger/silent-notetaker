//! Golden tests for the stop-time global recluster (docs/DIARIZATION.md §2).
//!
//! Each fixture is a "messy meeting": a synthetic embedding stream that the
//! online leader-clustering over-splits (same-speaker utterances dip below the
//! live threshold against the running centroid → phantom speakers). The
//! recluster compares robust full-meeting TRUE centroids and merges the phantoms
//! back. These tests:
//!
//! 1. Reproduce the reference relabel map exactly (Rust == JS).
//! 2. Quantify the improvement: pairwise label-error-rate BEFORE vs AFTER the
//!    recluster, against ground truth. The recluster must MEASURABLY reduce it.
//! 3. Verify renames survive the recluster (a PRD exit criterion).
//!
//! The pairwise metric (Rand-style) is the right one: it penalizes
//! over-splitting (breaking a same-speaker pair into two clusters is an error),
//! which a per-cluster majority-vote metric would not.
#![cfg(not(target_arch = "wasm32"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism (PRD lint config)"
)]
#![allow(
    clippy::cast_precision_loss,
    reason = "pairwise error/pair counts are small and exact as f64"
)]

use serde::Deserialize;
use silent_diarization::tracker::{SpeakerTracker, TrackerConfig};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Fixture {
    description: String,
    threshold: f32,
    #[serde(rename = "reclusterThreshold")]
    recluster_threshold: f32,
    steps: Vec<Step>,
    expected: Expected,
    truth: Vec<String>,
    accuracy: Accuracy,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Step {
    Identify { emb: Vec<f32> },
    Rename { id: String, name: String },
    Merge { from: String, to: String },
    Recluster { threshold: Option<f32> },
}

#[derive(Debug, Deserialize)]
struct Expected {
    #[serde(rename = "finalSpeakers")]
    final_speakers: Vec<FinalSpeaker>,
    #[serde(rename = "utteranceLabels")]
    utterance_labels: Vec<String>,
    trace: Vec<TraceItem>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum TraceItem {
    Identify {},
    Rename {},
    Merge {},
    Recluster {
        #[serde(default)]
        map: HashMap<String, String>,
    },
}

#[derive(Debug, Deserialize)]
struct FinalSpeaker {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Accuracy {
    pairwise_error_before: f64,
    pairwise_error_after: f64,
    n_speakers_before: usize,
    n_speakers_after: usize,
    n_speakers_true: usize,
}

fn load(rel: &str) -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens")
        .join(rel);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

/// Pairwise (Rand-style) label error against ground truth.
fn pairwise_error(assigned: &[String], truth: &[String]) -> f64 {
    let n = assigned.len();
    if n < 2 {
        return 0.0;
    }
    let mut errors = 0_usize;
    let mut pairs = 0_usize;
    for i in 0..n {
        for j in (i + 1)..n {
            pairs += 1;
            let same_assigned = assigned[i] == assigned[j];
            let same_truth = truth[i] == truth[j];
            if same_assigned != same_truth {
                errors += 1;
            }
        }
    }
    errors as f64 / pairs as f64
}

fn distinct(labels: &[String]) -> usize {
    labels
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

/// Run a recluster fixture and return the per-utterance labels just before the
/// recluster, the labels after it, the reference relabel map, and the final
/// tracker state.
fn run(
    fx: &Fixture,
) -> (
    Vec<String>,
    Vec<String>,
    HashMap<String, String>,
    SpeakerTracker,
) {
    let mut t = SpeakerTracker::new(TrackerConfig {
        live_threshold: fx.threshold,
        recluster_threshold: fx.recluster_threshold,
        ..TrackerConfig::default()
    });

    let mut labels_before: Vec<String> = Vec::new();
    let mut ref_map: HashMap<String, String> = HashMap::new();

    for (step, trace) in fx.steps.iter().zip(&fx.expected.trace) {
        match (step, trace) {
            (Step::Identify { emb }, TraceItem::Identify {}) => {
                t.identify_embedding(emb);
            }
            (Step::Rename { id, name }, TraceItem::Rename {}) => {
                t.rename(id, name);
            }
            (Step::Merge { from, to }, TraceItem::Merge {}) => {
                t.merge(from, to);
            }
            (Step::Recluster { threshold }, TraceItem::Recluster { map }) => {
                // Snapshot labels right before the recluster for the before/after
                // accuracy comparison.
                labels_before = t
                    .utterances()
                    .iter()
                    .map(|u| u.assigned_id.clone())
                    .collect();
                let got = t.global_recluster(*threshold);
                assert_eq!(
                    &got, map,
                    "[{}] recluster map must match the JS reference",
                    fx.description
                );
                ref_map = got;
            }
            (s, e) => panic!("[{}] step/trace mismatch: {s:?} vs {e:?}", fx.description),
        }
    }

    let labels_after: Vec<String> = t
        .utterances()
        .iter()
        .map(|u| u.assigned_id.clone())
        .collect();
    (labels_before, labels_after, ref_map, t)
}

/// Core assertion shared by the messy-meeting fixtures: recluster reproduces the
/// reference map AND measurably reduces pairwise error vs ground truth.
fn assert_recluster_improves(rel: &str) {
    let fx = load(rel);
    let (before, after, _map, tracker) = run(&fx);

    let err_before = pairwise_error(&before, &fx.truth);
    let err_after = pairwise_error(&after, &fx.truth);
    let n_before = distinct(&before);
    let n_after = distinct(&after);

    // Witness the numbers (visible with `cargo test -- --nocapture`).
    println!(
        "[recluster] {}\n  speakers: {} -> {} (true {})\n  pairwise-err: {:.4} -> {:.4}  (Δ {:.4})",
        fx.description,
        n_before,
        n_after,
        fx.accuracy.n_speakers_true,
        err_before,
        err_after,
        err_before - err_after,
    );

    // Cross-check against the values the JS reference recorded.
    assert!(
        (err_before - fx.accuracy.pairwise_error_before).abs() < 1e-9,
        "[{}] pairwise_error_before {err_before} != reference {}",
        fx.description,
        fx.accuracy.pairwise_error_before
    );
    assert!(
        (err_after - fx.accuracy.pairwise_error_after).abs() < 1e-9,
        "[{}] pairwise_error_after {err_after} != reference {}",
        fx.description,
        fx.accuracy.pairwise_error_after
    );
    assert_eq!(
        n_before, fx.accuracy.n_speakers_before,
        "[{}] n_before",
        fx.description
    );
    assert_eq!(
        n_after, fx.accuracy.n_speakers_after,
        "[{}] n_after",
        fx.description
    );

    // The headline claim: recluster measurably IMPROVES accuracy on a messy
    // meeting (strictly fewer pairwise errors AND fewer phantom speakers).
    assert!(
        err_after < err_before,
        "[{}] recluster must reduce pairwise error: {err_before} -> {err_after}",
        fx.description
    );
    assert!(
        n_after < n_before,
        "[{}] recluster must reduce speaker count: {n_before} -> {n_after}",
        fx.description
    );
    // It must not under-merge below the true count.
    assert!(
        n_after >= fx.accuracy.n_speakers_true,
        "[{}] recluster must not merge below the true speaker count",
        fx.description
    );

    // Final speaker set matches the reference exactly.
    assert_eq!(tracker.speakers().len(), fx.expected.final_speakers.len());
    for (got, exp) in tracker.speakers().iter().zip(&fx.expected.final_speakers) {
        assert_eq!(got.id, exp.id, "[{}] final id", fx.description);
        assert_eq!(got.name, exp.name, "[{}] final name", fx.description);
    }

    // Final per-utterance labels match the JS reference BYTE-FOR-BYTE. This is
    // stricter than the pairwise metric (which is invariant to relabeling) and
    // catches any divergence in how the recluster rewrites the utterance log
    // (the JS leaves survivor utterances on their pre-compaction id).
    let got_labels: Vec<&str> = tracker
        .utterances()
        .iter()
        .map(|u| u.assigned_id.as_str())
        .collect();
    let exp_labels: Vec<&str> = fx
        .expected
        .utterance_labels
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        got_labels, exp_labels,
        "[{}] utterance labels",
        fx.description
    );
}

#[test]
fn messy_two_speaker_recluster_repairs_over_split() {
    assert_recluster_improves("recluster/messy_two_speaker.json");
}

#[test]
fn messy_three_speaker_recluster_repairs_over_split() {
    assert_recluster_improves("recluster/messy_three_speaker.json");
}

/// PRD exit criterion: a rename made before the recluster MUST survive it. The
/// fixture renames an over-split speaker "Alice"; after the recluster merges its
/// phantoms, the surviving canonical speaker must still be named "Alice".
#[test]
fn recluster_preserves_rename() {
    let fx = load("recluster/recluster_preserves_rename.json");
    let (before, after, map, tracker) = run(&fx);

    // The recluster actually merged something (a real merge exercised the path).
    assert!(
        map.iter().any(|(k, v)| k != v),
        "[{}] fixture must exercise a real merge",
        fx.description
    );
    // Improvement still holds.
    let err_before = pairwise_error(&before, &fx.truth);
    let err_after = pairwise_error(&after, &fx.truth);
    println!(
        "[recluster] {} — pairwise-err {:.4} -> {:.4}",
        fx.description, err_before, err_after
    );
    assert!(
        err_after <= err_before,
        "[{}] rename fixture should not regress",
        fx.description
    );

    // The "Alice" name survives onto a surviving speaker.
    let alice = tracker.speakers().iter().find(|s| s.name == "Alice");
    assert!(
        alice.is_some(),
        "[{}] the Alice rename must survive the recluster",
        fx.description
    );
    // And it matches the reference final state.
    let ref_alice = fx
        .expected
        .final_speakers
        .iter()
        .find(|s| s.name == "Alice");
    assert!(ref_alice.is_some(), "reference should keep Alice");
    assert_eq!(
        alice.unwrap().id,
        ref_alice.unwrap().id,
        "[{}] Alice must land on the same canonical id as the reference",
        fx.description
    );
}
