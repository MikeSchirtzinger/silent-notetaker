//! Notes, smart-questions, and corrections policy (Phase 3).
//!
//! Houses the `NoteExtractor` trigger policy (decisions / actions / key points /
//! open questions — ported with byte-identical goldens from the current regexes
//! before any improvement), open-question tracking, word-correction application,
//! smart-question scheduling ([`questions`]: type rotation, reroll, recap,
//! new-question badge), and the Qwen final-notes pipeline ([`qwen`]: ~500-char
//! chunking up to 22 chunks, dedup, `TAG|` parsing) driving the Qwen worker
//! through the typed boundary. The worker stays the executor; this crate is the
//! policy. The notes slot is **optional**: the orchestrator handles `None`
//! (transcript-only mode) as a first-class state (PRD R3 /
//! `silent_core::engine::NotesEngine`).
#![forbid(unsafe_code)]

pub mod questions;
pub mod qwen;
