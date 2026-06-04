//! Audio primitives for Silent Notetaker.
//!
//! # The two mel frontends (do NOT unify)
//!
//! This crate hosts **two** mel front-ends that are different on nearly every
//! axis. They share a [`MelConfig`] *shape* but are instantiated with distinct,
//! named presets — they are not, and must never become, one configurable code
//! path that silently flips a window flag:
//!
//! | Property       | TitaNet ([`titanet_config`]) | Nemotron ([`nemotron_config`]) |
//! |----------------|------------------------------|--------------------------------|
//! | Mel bands      | 80, slaney                   | 128, slaney                    |
//! | Hann window    | **periodic** (`cos(2πn/WIN)`) | symmetric (`cos(2πn/(WIN-1))`) |
//! | Spectrum       | magnitude → power            | power                          |
//! | Normalization  | per-feature mean/std         | none                           |
//! | Filterbank     | pre-baked `mel_fb.json`      | computed at runtime            |
//!
//! Using the symmetric window with TitaNet drops the embedding cosine from
//! 1.000000 to ~0.9997 — a measurable mismatch that degrades clustering. The
//! periodic-Hann guard comments in [`mel`] document why. See
//! `docs/research/spike-titanet.md`.
//!
//! The TitaNet front-end is validated against the JS reference
//! (`eval/js/validate.mjs`) at **cosine 1.000000** (spike b1).
//!
//! This crate is pure Rust (`ndarray` + `realfft`); it has no `ort`, no
//! `wasm-bindgen`, and no browser dependency, so it compiles unchanged for
//! `wasm32-unknown-unknown`.
#![forbid(unsafe_code)]

pub mod error;
pub mod mel;

pub use error::{AudioError, Result};
pub use mel::{
    HannWindow, MelConfig, MelFrontend, NormalizationMode, SpectrumMode, nemotron_config,
    titanet_config,
};
