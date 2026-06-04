//! Golden parity test for the Qwen final-notes policy.
//!
//! Loads `goldens/qwen/golden_qwen.json` — produced by the DOM-free JS reference
//! generator `goldens/qwen/reference/qwen_reference.mjs`, whose functions are
//! copied verbatim from the shipping `index.html` — and asserts the Rust port in
//! `silent_notes::qwen` produces byte-identical output for every fixture case.
//!
//! This is the parity contract for Appendix A row 19. If the JS behavior ever
//! changes, regenerate the golden (`node qwen_reference.mjs`) and this test will
//! flag any Rust divergence.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a golden test uses unwrap/expect as its assertion mechanism (PRD lint config)"
)]

use serde::Deserialize;
use silent_core::questions::QwenNote;
use silent_notes::qwen::{chunk_transcript, dedupe_notes, final_notes_chunks, parse_qwen_notes};

/// The JS golden note shape (`{ cat, text, topic }`). Mirrors [`QwenNote`] but is
/// its own type so the JSON field layout the JS emits is pinned independently of
/// the Rust struct's serde.
#[derive(Debug, Deserialize, PartialEq)]
struct GoldenNote {
    cat: String,
    text: String,
    topic: Option<String>,
}

impl From<&QwenNote> for GoldenNote {
    fn from(n: &QwenNote) -> Self {
        GoldenNote {
            cat: n.cat.clone(),
            text: n.text.clone(),
            topic: n.topic.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Golden {
    parse_qwen_notes: std::collections::BTreeMap<String, ParseCase>,
    chunk_transcript: std::collections::BTreeMap<String, ChunkCase>,
    dedupe_notes: std::collections::BTreeMap<String, DedupCase>,
    final_notes_chunks: std::collections::BTreeMap<String, FinalChunkCase>,
}

#[derive(Debug, Deserialize)]
struct ParseCase {
    input: String,
    output: Vec<GoldenNote>,
}

#[derive(Debug, Deserialize)]
struct ChunkCase {
    text: String,
    target: usize,
    output: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DedupCase {
    input: Vec<GoldenNote>,
    output: Vec<GoldenNote>,
}

#[derive(Debug, Deserialize)]
struct FinalChunkCase {
    // The full `transcript` is not in the golden (only its length + the chunks);
    // we reconstruct each case's transcript by name from the same constructions
    // the JS reference uses, then assert its length equals `transcript_len` to
    // prove the Rust and JS inputs are identical before comparing `output`.
    transcript_len: usize,
    target: usize,
    output: Vec<String>,
}

fn load_golden() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/goldens/qwen/golden_qwen.json");
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read golden {path}: {e}; run `node qwen_reference.mjs`"));
    serde_json::from_str(&raw).expect("parse golden_qwen.json")
}

#[test]
fn parse_qwen_notes_matches_golden() {
    let golden = load_golden();
    assert!(
        !golden.parse_qwen_notes.is_empty(),
        "golden has no parse cases"
    );
    for (name, case) in &golden.parse_qwen_notes {
        let got: Vec<GoldenNote> = parse_qwen_notes(&case.input)
            .iter()
            .map(GoldenNote::from)
            .collect();
        assert_eq!(
            got, case.output,
            "parse_qwen_notes mismatch on case `{name}`"
        );
    }
}

#[test]
fn chunk_transcript_matches_golden() {
    let golden = load_golden();
    assert!(!golden.chunk_transcript.is_empty(), "no chunk cases");
    for (name, case) in &golden.chunk_transcript {
        let got = chunk_transcript(&case.text, case.target);
        assert_eq!(
            got, case.output,
            "chunk_transcript mismatch on case `{name}`"
        );
    }
}

#[test]
fn dedupe_notes_matches_golden() {
    let golden = load_golden();
    assert!(!golden.dedupe_notes.is_empty(), "no dedup cases");
    for (name, case) in &golden.dedupe_notes {
        let input: Vec<QwenNote> = case
            .input
            .iter()
            .map(|n| QwenNote {
                cat: n.cat.clone(),
                text: n.text.clone(),
                topic: n.topic.clone(),
            })
            .collect();
        let got: Vec<GoldenNote> = dedupe_notes(input).iter().map(GoldenNote::from).collect();
        assert_eq!(got, case.output, "dedupe_notes mismatch on case `{name}`");
    }
}

/// Rebuild the exact transcripts the reference generator fed each
/// `final_notes_chunks` case, so we can assert the Rust output matches the
/// golden chunk output. These constructions are copied from
/// `qwen_reference.mjs` `FINAL_CHUNK_CASES`.
fn final_chunk_transcript(name: &str) -> String {
    use std::fmt::Write as _;
    match name {
        "short_uses_500_floor" => "Short transcript. Just two sentences.".to_owned(),
        "long_grows_target" => {
            let mut s = String::new();
            for i in 1..=60 {
                let _ = write!(
                    s,
                    "Sentence number {i} carries a substantive point worth recording for the record. "
                );
            }
            s.trim().to_owned()
        }
        "caps_at_22" => {
            let mut s = String::new();
            for i in 1..=30 {
                let _ = write!(s, "S{i} {}. ", "z".repeat(280));
            }
            s.trim().to_owned()
        }
        other => panic!("unknown final_notes_chunks case `{other}` — sync with qwen_reference.mjs"),
    }
}

#[test]
fn final_notes_chunks_matches_golden() {
    let golden = load_golden();
    assert!(
        !golden.final_notes_chunks.is_empty(),
        "no final-chunk cases"
    );
    for (name, case) in &golden.final_notes_chunks {
        let transcript = final_chunk_transcript(name);
        // Sanity: the transcript we reconstructed must match the length the JS
        // reference recorded, proving we fed the SAME input.
        assert_eq!(
            transcript.chars().count(),
            case.transcript_len,
            "reconstructed transcript length mismatch on `{name}` — \
             the Rust and JS reference inputs have diverged"
        );
        let got = final_notes_chunks(&transcript);
        assert_eq!(
            got, case.output,
            "final_notes_chunks mismatch on case `{name}` (target {})",
            case.target
        );
        assert!(
            got.len() <= 22,
            "final_notes_chunks must cap at 22 (case `{name}` produced {})",
            got.len()
        );
    }
}
