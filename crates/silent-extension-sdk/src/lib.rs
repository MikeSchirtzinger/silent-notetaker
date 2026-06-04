//! Extension SDK (stub — Phase 6).
//!
//! Will house the capability-based, network-denied-by-default extension system
//! (PRD R7): the manifest schema, capability enforcement, and host messages.
//! Raw audio, embeddings, mel tensors, and model activations are **not**
//! grantable capabilities — they are not in the vocabulary, so an extension
//! cannot request them (PRD R3/R7). Network grants are origin-scoped, visible
//! at install, and revocable. No third-party extensions ship until CSP is
//! enforced (not report-only).
//!
//! Empty by design (Task C1 scaffold).
#![forbid(unsafe_code)]
