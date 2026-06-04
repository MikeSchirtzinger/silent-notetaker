//! Notes, smart-questions, and corrections policy (stub — Phase 3).
//!
//! Will house the `NoteExtractor` trigger policy (decisions / actions / key
//! points / open questions — ported with byte-identical goldens from the
//! current regexes before any improvement), open-question tracking,
//! word-correction application, and smart-question scheduling (type rotation,
//! reroll, recap) driving the Qwen worker through the typed boundary. The
//! notes slot is **optional**: the orchestrator handles `None` (transcript-only
//! mode) as a first-class state (PRD R3 / `silent_core::engine::NotesEngine`).
//!
//! Empty by design (Task C1 scaffold).
#![forbid(unsafe_code)]
