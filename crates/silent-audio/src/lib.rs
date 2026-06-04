//! Audio primitives for Silent Notetaker (stub — Phase 2+).
//!
//! Will house ring buffers, resampling, the consolidated chunking core (Task
//! D3), and the **two** parameterized mel frontends (PRD "Validation plan"):
//! the TitaNet 80-band slaney / periodic-Hann / per-feature-normalized frontend
//! and the Nemotron 128-band slaney / symmetric-Hann / power-spectrum frontend.
//! These are different on nearly every axis and must never be unified — a PR
//! that "deduplicates" them is a correctness bug (see
//! `docs/research/spike-titanet.md`).
//!
//! Empty by design: the workspace scaffold (Task C1) reserves this crate so
//! Phase D agents can fill it without touching the root `Cargo.toml`.
#![forbid(unsafe_code)]
