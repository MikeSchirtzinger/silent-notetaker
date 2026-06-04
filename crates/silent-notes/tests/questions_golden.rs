//! Golden parity test for the smart-question scheduling policy.
//!
//! Loads `goldens/questions/golden_questions.json` — produced by the DOM-free JS
//! reference generator `goldens/questions/reference/questions_reference.mjs`,
//! whose `SmartQ` policy logic is copied from the shipping `index.html` — and
//! asserts the Rust port in `silent_notes::questions` reproduces the same
//! behavioral trace for every scenario, plus the same `_norm` and recap-cleaning
//! output.
//!
//! Driving model: the JS reference is synchronous (the model reply is scripted),
//! so each generating step yields ONE result record with the full attempt loop.
//! The Rust `Scheduler` is async-shaped (`accumulate`/`reroll` issue a request;
//! `on_worker_result` processes the reply and may retry). This harness drives the
//! Rust scheduler exactly as the orchestrator+worker would — feeding the scripted
//! replies through `on_worker_result` until a question is Ready — and assembles a
//! result record in the golden's shape for comparison. That is the parity proof:
//! the Rust policy, driven the real way, reproduces the JS trace.
//!
//! Parity contract for Appendix A row 21.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a golden test uses unwrap/expect as its assertion mechanism (PRD lint config)"
)]

use serde::Deserialize;
use silent_core::questions::{QuestionEvent, QuestionType};
use silent_notes::questions::{
    RerollOutcome, ScheduleOutcome, Scheduler, WorkerOutcome, norm, recap_clean_group,
};
use std::collections::BTreeMap;

// ── Golden JSON shapes ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Golden {
    norm: BTreeMap<String, NormCase>,
    scheduling: BTreeMap<String, Scenario>,
    recap_clean: BTreeMap<String, RecapCase>,
}

#[derive(Debug, Deserialize)]
struct NormCase {
    input: String,
    output: String,
}

#[derive(Debug, Deserialize)]
struct RecapCase {
    input: String,
    output: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    enabled_types: Vec<String>,
    trace: Vec<TraceStep>,
}

/// One step's recorded result, as the reference emits it. Only the fields we
/// assert on are deserialized; the rest of the JSON is ignored.
#[derive(Debug, Deserialize)]
struct TraceStep {
    op: String,
    #[serde(default)]
    result: serde_json::Value,
}

// ── Scenario step DSL (re-encodes qwen_reference.mjs SCHED steps in Rust) ────

/// A scripted scenario step. The replies are the model outputs the worker would
/// return for up to two attempts (the reference's `replies` array).
enum Step {
    Accumulate {
        text: String,
        now: u64,
        replies: Vec<String>,
    },
    Reroll {
        now: u64,
        replies: Vec<String>,
    },
    Toggle,
    Reset,
}

/// 120-char filler helper, identical to the reference `C(n)` (`'word '` repeated
/// then sliced to `n`).
fn c(n: usize) -> String {
    let mut s = String::new();
    while s.len() < n {
        s.push_str("word ");
    }
    s.truncate(n);
    s
}

fn type_label(t: QuestionType) -> &'static str {
    match t {
        QuestionType::Clarify => "clarify",
        QuestionType::Risk => "risk",
        QuestionType::Followup => "followup",
        QuestionType::Coverage => "coverage",
        QuestionType::Deepen => "deepen",
        // `QuestionType` is `#[non_exhaustive]`; the five above are the full
        // rotation. A new variant would be a deliberate boundary change that
        // this test should be updated for.
        _ => panic!("unhandled QuestionType variant"),
    }
}

fn parse_types(labels: &[String]) -> Vec<QuestionType> {
    labels
        .iter()
        .map(|l| match l.as_str() {
            "clarify" => QuestionType::Clarify,
            "risk" => QuestionType::Risk,
            "followup" => QuestionType::Followup,
            "coverage" => QuestionType::Coverage,
            "deepen" => QuestionType::Deepen,
            other => panic!("unknown type label `{other}`"),
        })
        .collect()
}

/// Reproduce the reference scenarios by name (mirrors `qwen_reference.mjs` SCHED).
fn scenario_steps(name: &str) -> Vec<Step> {
    let acc = |text: String, now: u64, replies: &[&str]| Step::Accumulate {
        text,
        now,
        replies: replies.iter().map(|s| (*s).to_owned()).collect(),
    };
    match name {
        "short_window_no_gen" => vec![acc(c(50), 1000, &["Q1?"])],
        "first_generation" => vec![acc(c(240), 5000, &["What is the deadline?"])],
        "type_rotation" => vec![
            acc(c(240), 0, &["Q1 unique?"]),
            acc(c(240), 61_000, &["Q2 unique?"]),
            acc(c(240), 122_000, &["Q3 unique?"]),
        ],
        "dedup_retry" => vec![
            acc(c(240), 0, &["Repeat question?"]),
            acc(c(240), 61_000, &["Repeat question?", "Fresh question?"]),
        ],
        "dedup_both_dupe" => vec![
            acc(c(240), 0, &["Only one?"]),
            acc(c(240), 61_000, &["Only one?", "Only one?"]),
        ],
        "time_gate_blocks" => vec![
            acc(c(240), 1000, &["First?"]),
            acc(c(240), 30_000, &["Too soon?"]),
        ],
        "char_gate_blocks" => vec![
            acc(c(240), 0, &["First?"]),
            acc(c(100), 120_000, &["Not enough chars?"]),
        ],
        "window_cap" => vec![acc("A".repeat(1500), 0, &["Capped?"])],
        "reroll_forces_gen" => vec![
            acc(c(240), 0, &["First?"]),
            Step::Reroll {
                now: 5000,
                replies: vec!["Rerolled?".to_owned()],
            },
        ],
        "toggle_clears_badge" => vec![acc(c(240), 0, &["First?"]), Step::Toggle, Step::Toggle],
        "reset_clears" => vec![
            acc(c(240), 0, &["Q?"]),
            Step::Reset,
            acc(c(50), 1000, &["too short?"]),
        ],
        "subset_types" => vec![
            acc(c(240), 0, &["Q1?"]),
            acc(c(240), 61_000, &["Q2?"]),
            acc(c(240), 122_000, &["Q3?"]),
        ],
        "recent_ring_evicts" => (0..10)
            .map(|i| Step::Accumulate {
                text: c(240),
                now: i * 61_000,
                replies: vec![format!("Unique question number {i}?")],
            })
            .collect(),
        other => panic!("unknown scenario `{other}` — sync with questions_reference.mjs"),
    }
}

// ── Driving the Rust scheduler to reproduce one generating step's record ─────

/// The reconstructed "result" record for one generating step, in the golden's
/// shape (only the fields we assert on).
struct StepRecord {
    action: String,
    attempts: Vec<(usize, String, String)>, // (attempt, type, question)
    chosen_type: Option<String>,
    chosen_question: Option<String>,
    recent_after: Vec<String>,
    badge_has_new: bool,
    win_len: usize,
}

/// Run a worker-reply loop after a [`ScheduleOutcome`]/reroll request, feeding
/// the scripted replies, collecting the attempt records, until Ready.
fn drive_to_ready(
    sched: &mut Scheduler,
    first_request: QuestionEvent,
    replies: &[String],
) -> (Vec<(usize, String, String)>, String, String) {
    let mut attempts = Vec::new();
    let mut request = first_request;
    let mut attempt_idx = 0usize;
    loop {
        let (request_id, kind) = match &request {
            QuestionEvent::GenerateRequest {
                request_id, kind, ..
            } => (*request_id, *kind),
            other => panic!("expected GenerateRequest, got {other:?}"),
        };
        let reply = replies.get(attempt_idx).cloned().unwrap_or_default();
        attempts.push((attempt_idx, type_label(kind).to_owned(), reply.clone()));
        match sched.on_worker_result(request_id, &reply) {
            WorkerOutcome::Ready(QuestionEvent::QuestionReady { text, kind, .. }) => {
                return (attempts, type_label(kind).to_owned(), text);
            }
            WorkerOutcome::Retry(next) => {
                request = next;
                attempt_idx += 1;
            }
            other => panic!("unexpected worker outcome {other:?}"),
        }
    }
}

/// Replay one scenario through the Rust scheduler, producing a `StepRecord` per
/// step (or `None` for non-generating ops we assert separately).
fn replay(name: &str, enabled: &[QuestionType]) -> Vec<Option<StepRecord>> {
    let mut sched = Scheduler::new(enabled);
    let mut out = Vec::new();
    for step in scenario_steps(name) {
        match step {
            Step::Accumulate { text, now, replies } => match sched.accumulate(&text, now) {
                ScheduleOutcome::Accumulated => out.push(None),
                ScheduleOutcome::Generate { request } => {
                    let (attempts, chosen_type, chosen_question) =
                        drive_to_ready(&mut sched, request, &replies);
                    out.push(Some(record(
                        &sched,
                        "generated",
                        attempts,
                        chosen_type,
                        chosen_question,
                    )));
                }
            },
            Step::Reroll { now, replies } => {
                let RerollOutcome { outcome, .. } = sched.reroll(now);
                match outcome {
                    ScheduleOutcome::Generate { request } => {
                        let (attempts, chosen_type, chosen_question) =
                            drive_to_ready(&mut sched, request, &replies);
                        out.push(Some(record(
                            &sched,
                            "generated",
                            attempts,
                            chosen_type,
                            chosen_question,
                        )));
                    }
                    ScheduleOutcome::Accumulated => out.push(None),
                }
            }
            Step::Toggle => {
                sched.toggle_minimize();
                out.push(None);
            }
            Step::Reset => {
                sched.reset();
                out.push(None);
            }
        }
    }
    out
}

fn record(
    sched: &Scheduler,
    action: &str,
    attempts: Vec<(usize, String, String)>,
    chosen_type: String,
    chosen_question: String,
) -> StepRecord {
    StepRecord {
        action: action.to_owned(),
        attempts,
        chosen_type: Some(chosen_type),
        chosen_question: Some(chosen_question),
        recent_after: sched.recent_snapshot(),
        badge_has_new: sched.has_new_badge(),
        win_len: sched.window_len(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

fn load_golden() -> Golden {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/goldens/questions/golden_questions.json"
    );
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read golden {path}: {e}; run `node questions_reference.mjs`"));
    serde_json::from_str(&raw).expect("parse golden_questions.json")
}

#[test]
fn norm_matches_golden() {
    let golden = load_golden();
    assert!(!golden.norm.is_empty(), "no norm cases");
    for (name, case) in &golden.norm {
        assert_eq!(norm(&case.input), case.output, "norm mismatch on `{name}`");
    }
}

#[test]
fn recap_clean_matches_golden() {
    let golden = load_golden();
    assert!(!golden.recap_clean.is_empty(), "no recap cases");
    for (name, case) in &golden.recap_clean {
        assert_eq!(
            recap_clean_group(&case.input),
            case.output,
            "recap_clean_group mismatch on `{name}`"
        );
    }
}

#[test]
fn scheduling_traces_match_golden() {
    let golden = load_golden();
    assert!(!golden.scheduling.is_empty(), "no scheduling scenarios");
    for (name, scenario) in &golden.scheduling {
        let enabled = parse_types(&scenario.enabled_types);
        let got = replay(name, &enabled);
        assert_eq!(
            got.len(),
            scenario.trace.len(),
            "step count mismatch on scenario `{name}`"
        );
        for (i, (step, golden_step)) in got.iter().zip(scenario.trace.iter()).enumerate() {
            assert_step(name, i, step.as_ref(), golden_step);
        }
    }
}

fn assert_step(scenario: &str, i: usize, got: Option<&StepRecord>, golden: &TraceStep) {
    let ctx = format!("scenario `{scenario}` step {i} (op `{}`)", golden.op);
    let g = &golden.result;
    let g_action = g.get("action").and_then(|v| v.as_str());
    match got {
        // A non-generating Rust step (accumulate that did not fire, toggle,
        // reset, reroll-no-gen). The golden's matching step must NOT be a
        // generated record.
        None => {
            assert_ne!(
                g_action,
                Some("generated"),
                "{ctx}: golden generated but Rust did not"
            );
        }
        Some(rec) => {
            assert_eq!(
                g_action,
                Some(rec.action.as_str()),
                "{ctx}: action mismatch"
            );
            // attempts: [{attempt, type, question}]
            let g_attempts = g.get("attempts").and_then(|v| v.as_array());
            let g_attempts = g_attempts.unwrap_or_else(|| panic!("{ctx}: golden missing attempts"));
            assert_eq!(
                rec.attempts.len(),
                g_attempts.len(),
                "{ctx}: attempt count mismatch"
            );
            for (a, ga) in rec.attempts.iter().zip(g_attempts.iter()) {
                assert_eq!(
                    a.1.as_str(),
                    ga.get("type").and_then(|v| v.as_str()).unwrap(),
                    "{ctx}: attempt {} type mismatch",
                    a.0
                );
                assert_eq!(
                    a.2.as_str(),
                    ga.get("question").and_then(|v| v.as_str()).unwrap(),
                    "{ctx}: attempt {} question mismatch",
                    a.0
                );
            }
            assert_eq!(
                rec.chosen_type.as_deref(),
                g.get("chosen_type").and_then(|v| v.as_str()),
                "{ctx}: chosen_type mismatch"
            );
            assert_eq!(
                rec.chosen_question.as_deref(),
                g.get("chosen_question").and_then(|v| v.as_str()),
                "{ctx}: chosen_question mismatch"
            );
            let g_recent: Vec<String> = g
                .get("recent_after")
                .and_then(|v| v.as_array())
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(rec.recent_after, g_recent, "{ctx}: recent_after mismatch");
            assert_eq!(
                rec.badge_has_new,
                g.get("badge_has_new")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap(),
                "{ctx}: badge_has_new mismatch"
            );
            assert_eq!(
                rec.win_len as u64,
                g.get("win_len")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap(),
                "{ctx}: win_len mismatch"
            );
        }
    }
}
