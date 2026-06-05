//! Wasm-bindgen registry-driven engine-selection surface (PRD Phase 5, Task I3;
//! Appendix A rows 7, 8).
//!
//! The thin browser boundary over [`silent_inference::selection`] — the pure-Rust
//! selection policy. The JS glue (`selection-engine.js`) hands in a typed device
//! probe and gets back:
//!
//! - the settings ASR picker option list, sourced from the embedded registry's
//!   `ui` entries ([`asr_picker_options`]) — every shipping engine (incl.
//!   Nemotron) exactly as today, plus row-8 backend/precision data per option;
//! - per-engine availability verdicts with reasons + a recommended CPU-tier
//!   alternative ([`Availability`]) — never a silent fallback (R1);
//! - the per-tier recommended default ([`recommended_default`]) the UI may show
//!   (user choice always wins — R3);
//! - the queued mid-recording switch outcome ([`apply_selection`]) — a friendly
//!   "takes effect next meeting" notice, never a silent failure or hard reject
//!   (R3 decision log).
//!
//! # Wire format
//!
//! Inputs/outputs cross as JSON strings the glue `JSON.parse`s / `JSON.stringify`s
//! — the same serde-JSON convention as [`crate::exports`] and the `dedupeNotes`
//! free functions in [`crate::notes`]. No `serde-wasm-bindgen` dependency is
//! pulled in here; the policy crate is browser-free, so all of this is a thin
//! serialize hop.
//!
//! # No new egress
//!
//! The registry is embedded in the wasm binary (the embed-vs-deploy decision is
//! documented in `silent_inference::selection`), so this surface performs no
//! network I/O and adds no CSP `connect-src` entry. Privacy: nothing crosses a
//! network boundary here.
//!
//! # wasm32-only
//!
//! Compiled only for `wasm32-unknown-unknown`; the native workspace build gates
//! this module out (see `lib.rs`).

use wasm_bindgen::prelude::*;

use silent_inference::selection::{self, DeviceProbe};

/// Build a `JsError` from any `Display` error (a loud failure, never a silent
/// drop).
fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Serialize a value to a JSON string `JsValue` (the glue `JSON.parse`s it).
fn to_js_value<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(v).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

/// Parse the JSON device probe the glue assembled from `navigator` /
/// `crossOriginIsolated` into the typed [`DeviceProbe`].
fn parse_probe(probe_json: &str) -> Result<DeviceProbe, JsError> {
    serde_json::from_str(probe_json).map_err(to_js_err)
}

/// The settings ASR model picker option list, registry-driven, with per-engine
/// availability for the probed device (Appendix A rows 7, 8).
///
/// `probe_json` is a JSON `DeviceProbe` (`{ webgpu_available, memory_gb,
/// cross_origin_isolated, thread_count, max_gpu_buffer_gb }`). Returns a JSON
/// array of `PickerOption` (`{ value, label, model_id, backend, precision,
/// availability }`), ordered by the registry `ui.order` — exactly the shipping
/// row-7 list, plus the row-8 backend/precision data and the availability
/// verdict per option.
///
/// # Errors
///
/// Returns a `JsError` if `probe_json` is malformed or the embedded registry
/// fails to parse (a build-integrity failure).
#[wasm_bindgen(js_name = asrPickerOptions)]
pub fn asr_picker_options(probe_json: &str) -> Result<JsValue, JsError> {
    let probe = parse_probe(probe_json)?;
    let options = selection::asr_picker_options(&probe).map_err(to_js_err)?;
    to_js_value(&options)
}

/// Resolve a persisted picker `value` (a stored `settings.model`) to its option,
/// with availability for the probe.
///
/// `value` is the localStorage selection key; `probe_json` is the JSON
/// `DeviceProbe`. Returns the single `PickerOption` JSON.
///
/// # Errors
///
/// Returns a `JsError` if the value matches no registry engine (a stale/unknown
/// stored key), if `probe_json` is malformed, or if the registry fails to parse.
#[wasm_bindgen(js_name = resolveSelection)]
pub fn resolve_selection(value: &str, probe_json: &str) -> Result<JsValue, JsError> {
    let probe = parse_probe(probe_json)?;
    let option = selection::resolve_selection(value, &probe).map_err(to_js_err)?;
    to_js_value(&option)
}

/// The per-tier recommended default ASR engine for the probed device (the
/// registry `ui` engine marked default for the resolved tier and available),
/// falling back to the CPU-tier engine (Nemotron) when none is.
///
/// `probe_json` is the JSON `DeviceProbe`. Returns the recommended picker `value`
/// as a JSON string, or JSON `null` when none exists. The UI may surface this as
/// a recommendation; user choice always wins (R3).
///
/// # Errors
///
/// Returns a `JsError` if `probe_json` is malformed or the registry fails to
/// parse.
#[wasm_bindgen(js_name = recommendedDefault)]
pub fn recommended_default(probe_json: &str) -> Result<JsValue, JsError> {
    let probe = parse_probe(probe_json)?;
    let rec = selection::recommended_default(&probe).map_err(to_js_err)?;
    to_js_value(&rec)
}

/// The resolved device tier for a probe (`wasm_only` / `webgpu_low` /
/// `webgpu_mid` / `webgpu_high`) — the generalized Qwen tier mechanism (R3).
///
/// `probe_json` is the JSON `DeviceProbe`. Returns the tier as a JSON string.
/// Exposed so the UI can show the detected tier (parity with the shipping
/// `GpuCaps.detail`).
///
/// # Errors
///
/// Returns a `JsError` if `probe_json` is malformed.
#[wasm_bindgen(js_name = resolveTier)]
pub fn resolve_tier(probe_json: &str) -> Result<JsValue, JsError> {
    let probe = parse_probe(probe_json)?;
    to_js_value(&selection::resolve_tier(&probe))
}

/// The queued mid-recording switch policy: decide whether a selection change
/// applies now (idle) or is queued for the next meeting (recording), with the
/// friendly notice (R3 decision log).
///
/// `new_value` is the picker `value` the user chose; `is_recording` is whether a
/// recording is in progress. Returns a JSON `SwitchOutcome`
/// (`{ outcome: "applied_now", value }` or
/// `{ outcome: "queued_for_next_meeting", value, notice }`). The caller persists
/// the selection regardless (user choice wins); this only decides *when* it takes
/// effect and what notice to show — never a silent failure, never a hard reject.
///
/// # Errors
///
/// Returns a `JsError` only if the outcome fails to serialize (it cannot in
/// practice; the `Result` keeps the surface uniform).
#[wasm_bindgen(js_name = applySelection)]
pub fn apply_selection(new_value: &str, is_recording: bool) -> Result<JsValue, JsError> {
    to_js_value(&selection::apply_selection(new_value, is_recording))
}
