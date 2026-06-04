//! Engine host adapters and selection policy.
//!
//! Will implement the concrete [`silent_core::engine::AnyAsrEngine`] variants:
//! the `rust-ort-web` host (Nemotron, TitaNet), the `js-transformers` host
//! (Voxtral, Whisper, Moonshine, Qwen) driven over the typed command protocol,
//! and the `js-sherpa` host (SenseVoice). Policy — chunk sizes, Voxtral's
//! two-cap recycle, when to feed/recycle — stays here in Rust; the JS workers
//! only execute (PRD R2). Registry-driven selection + device tiers land here
//! later (Task I3).
//!
//! # What's implemented (Task I1)
//!
//! [`voxtral_recycle`] — Voxtral's token/audio **two-cap context recycle**, the
//! hardest-won bug fix in the app (PRD Appendix A row 10; the JS source is
//! `index.html` `_runVoxtralTranscription`). It moves from a JS closure into a
//! deterministic, natively unit-tested Rust policy module (PRD R2). The policy
//! emits typed [`voxtral_recycle::HostCommand`]s; the transformers.js worker
//! (later wiring, in `silent-web`) is the executor and holds **no policy** —
//! exactly the `JsHostEngine` boundary the b2 spike measured (`docs/research/
//! spike-jshost.md`: no hot-path latency regression).
#![forbid(unsafe_code)]

pub mod voxtral_recycle;
