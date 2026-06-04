//! Engine host adapters and selection policy (stub — Phase 5).
//!
//! Will implement the concrete [`silent_core::engine::AnyAsrEngine`] variants:
//! the `rust-ort-web` host (Nemotron, TitaNet), the `js-transformers` host
//! (Voxtral, Whisper, Moonshine, Qwen) driven over the typed command protocol,
//! and the `js-sherpa` host (SenseVoice). Policy — chunk sizes, Voxtral's
//! two-cap recycle, when to feed/recycle — stays here in Rust; the JS workers
//! only execute (PRD R2). Registry-driven selection + device tiers also land
//! here (Task I3).
//!
//! Empty by design (Task C1 scaffold).
#![forbid(unsafe_code)]
