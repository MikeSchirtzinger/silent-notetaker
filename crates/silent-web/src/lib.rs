//! The wasm-bindgen UI↔core boundary (stub — Phase 3+).
//!
//! Will host the `wasm-bindgen` exports that receive [`silent_core::commands`]
//! from the UI and emit [`silent_core::commands::SessionEvent`] back, plus the
//! glue that drives the JS host workers (`transformers-host.js`,
//! `ort-web-loader.js`) and the bridge client. It depends on `silent-core` and
//! re-exports its types across the wasm boundary.
//!
//! Per the A3 spike, the **TypeScript type definitions** are generated from
//! `silent-core`'s types via `cargo test -p silent-core export_bindings`
//! (committed to `crates/silent-core/bindings/`), not from this crate — keeping
//! type generation decoupled from the (slow) wasm build. This crate is the
//! runtime boundary; `silent-core` owns the type contract.
//!
//! Empty by design (Task C1 scaffold).
#![forbid(unsafe_code)]
