//! Wasm-bindgen diarization surface (PRD Phase 2, Task F2).
//!
//! Exposes the combined TitaNet embedder + SpeakerTracker to the browser UI.
//! The UI loads this as an ES module (from `crates/silent-web/pkg/`), following
//! the same pattern as `nemotron-engine.js` wraps `crates/nemotron-asr/pkg/`.
//!
//! # Commands in, events out
//!
//! The JS glue (`diarization-engine.js`) calls these methods; they return
//! serde-JSON values that the glue deserializes into the typed
//! `DiarizationEvent` shapes defined in `silent-core/src/diarization.rs`.
//!
//! # Privacy (PRD R5)
//!
//! Raw embeddings (192-d `Float32Array`) are computed entirely inside Rust and
//! stored only in the tracker's utterance log (`silent-diarization` state).
//! They never appear in any return value; only `SpeakerDescriptor` (id, name,
//! color, count) crosses the wasm boundary. Verified by the resource-list check
//! in the F2 validation step.
//!
//! # wasm32-only
//!
//! This module is compiled only when targeting `wasm32-unknown-unknown`.
//! The native workspace build includes it as dead-code-free by the `cfg` gate
//! on the `use` in `lib.rs`.

use silent_diarization::embedder_web::WasmTitaNetEmbedder;
use silent_diarization::{RenameOutcome, Speaker, SpeakerTracker, TrackerConfig};

use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Serialize a value to a `JsValue` via serde-json.
///
/// Returns a `JsError` on serialization failure (should be impossible for our
/// well-formed types, but we never panic in production paths).
fn to_js_value<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let s = serde_json::to_string(v).map_err(to_js_err)?;
    Ok(JsValue::from_str(&s))
}

// ---------------------------------------------------------------------------
// The combined diarization object exposed to JS
// ---------------------------------------------------------------------------

/// Browser-facing diarization surface: TitaNet embedder + SpeakerTracker.
///
/// # Lifecycle (mirrors nemotron-engine.js)
///
/// 1. `WasmDiarization.create(onnx_bytes, mel_fb_bytes, dist_base_url)` — loads
///    the ort-web runtime and builds the ONNX session. Async.
/// 2. `identify(samples)` — embed + track, returns a JSON-serialized
///    `DiarizationEvent::SpeakerAssigned` (or `null` for the too-short branch).
/// 3. `reuse_last_speaker()` — the too-short segment branch; returns a JSON
///    `DiarizationEvent::SpeakerAssigned` or `null` if no prior speaker.
/// 4. `evaluate_rename(from_id, value)` — returns a JSON `RenameOutcome`
///    (`{ "Rename": ... }` or `{ "Merge": ... }`). The UI owns the confirm
///    dialog; on yes it calls `confirm_merge`.
/// 5. `confirm_merge(from_id, to_id)` — applies the merge; returns a JSON
///    `DiarizationEvent::MergeApplied`.
/// 6. `rename(id, name)` — plain rename; returns a JSON
///    `DiarizationEvent::SpeakerRenamed`.
/// 7. `global_recluster(threshold)` — stop-time recluster; returns a JSON
///    `DiarizationEvent::Reclustered`.
/// 8. `speakers()` — snapshot of the current speaker list as a JSON array of
///    `SpeakerDescriptor`. Used to rebuild the speakers bar.
#[wasm_bindgen]
pub struct WasmDiarization {
    embedder: WasmTitaNetEmbedder,
    tracker: SpeakerTracker,
    /// Configured minimum sample count for a confident embedding. Segments
    /// shorter than this reuse the last speaker (the JS `minSamples = 16000`
    /// branch). Held here so the combined object owns the policy.
    min_samples: usize,
}

#[wasm_bindgen]
impl WasmDiarization {
    /// Create the diarization surface, loading ort-web from the same CDN origin
    /// the app already fetches it from (currently `cdn.pyke.io`, in the CSP).
    ///
    /// - `onnx_bytes`: the TitaNet-small ONNX model bytes.
    /// - `mel_fb_json`: the 80×257 slaney mel filterbank, as UTF-8 JSON.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the ort-web runtime, ONNX session, or mel
    /// filterbank cannot be initialised.
    pub async fn create(onnx_bytes: &[u8], mel_fb_json: &[u8]) -> Result<WasmDiarization, JsError> {
        console_error_panic_hook::set_once();
        let embedder = WasmTitaNetEmbedder::create(onnx_bytes, mel_fb_json)
            .await
            .map_err(|e| JsError::new(&format!("{e:?}")))?;
        Ok(WasmDiarization {
            embedder,
            tracker: SpeakerTracker::new(TrackerConfig::default()),
            min_samples: TrackerConfig::default().min_samples,
        })
    }

    /// Create the diarization surface, loading ort-web from a same-origin
    /// vendored base URL (e.g. `"./vendor/"`). Preferred when
    /// `crossOriginIsolated === true` and the runtime is vendored same-origin
    /// (B3 recommendation).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the vendored ort-web runtime, ONNX session, or
    /// mel filterbank cannot be initialised.
    pub async fn create_with_dist(
        onnx_bytes: &[u8],
        mel_fb_json: &[u8],
        dist_base_url: &str,
    ) -> Result<WasmDiarization, JsError> {
        console_error_panic_hook::set_once();
        let embedder =
            WasmTitaNetEmbedder::create_with_dist(onnx_bytes, mel_fb_json, dist_base_url)
                .await
                .map_err(|e| JsError::new(&format!("{e:?}")))?;
        Ok(WasmDiarization {
            embedder,
            tracker: SpeakerTracker::new(TrackerConfig::default()),
            min_samples: TrackerConfig::default().min_samples,
        })
    }

    /// Embed + track a segment of 16 kHz mono PCM. This is the hot path called
    /// on every utterance boundary.
    ///
    /// - If `samples.length < min_samples` (default 16 000), calls
    ///   `reuse_last_speaker` instead (the JS `minSamples` branch).
    /// - On embedder failure, returns `null` (honest degradation — no fake labels).
    ///
    /// Returns a JSON-serialized object:
    /// ```json
    /// { "id": "S1", "name": "", "color": "#00d4aa", "is_new": true }
    /// ```
    /// or `null`.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure (should not occur
    /// for these well-typed structs).
    pub async fn identify(&mut self, samples: &[f32]) -> Result<JsValue, JsError> {
        // Too-short segment: reuse last speaker, no embedding.
        if samples.len() < self.min_samples {
            return self.reuse_last_speaker();
        }

        let emb = match self.embedder.embed(samples).await {
            Ok(e) => e,
            Err(e) => {
                // Log the failure; return null so the UI can degrade gracefully
                // (matching the JS `this.available = false; return null` path).
                web_sys::console::warn_1(
                    &format!("[rust-diarization] embed failed (disabling): {e:?}").into(),
                );
                return Ok(JsValue::null());
            }
        };

        let identified = self.tracker.identify_embedding(&emb);
        let result = IdentifiedResult {
            id: identified.id,
            name: identified.name,
            color: identified.color,
            is_new: identified.is_new,
        };
        to_js_value(&result)
    }

    /// Return the last-assigned speaker without running the embedder. Called
    /// when the segment is too short for a confident embedding (the JS
    /// `lastSpeakerId` branch).
    ///
    /// Returns a JSON `{ "id": "S1", "name": "", "color": "#00d4aa", "is_new": false }`
    /// or `null` if there is no prior speaker.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn reuse_last_speaker(&mut self) -> Result<JsValue, JsError> {
        match self.tracker.reuse_last_speaker() {
            Some(identified) => {
                let result = IdentifiedResult {
                    id: identified.id,
                    name: identified.name,
                    color: identified.color,
                    is_new: false,
                };
                to_js_value(&result)
            }
            None => Ok(JsValue::null()),
        }
    }

    /// Evaluate whether a committed rename is really a merge-by-rename.
    ///
    /// Returns a JSON `RenameOutcome`:
    /// - `{ "tag": "merge", "payload": { "from": "S2", "target": "S1" } }` if
    ///   the value matches another speaker's id or name (the UI should confirm).
    /// - `{ "tag": "rename", "payload": { "id": "S1", "name": "Alice" } }` for
    ///   a plain rename.
    ///
    /// The UI owns the `confirm()` dialog; on yes it calls `confirm_merge`,
    /// on no it calls `rename` directly.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn evaluate_rename(&self, from_id: &str, value: &str) -> Result<JsValue, JsError> {
        let outcome = self.tracker.evaluate_rename(from_id, value);
        // Map to the snake_case tagged-enum JSON shape the UI consumes
        // (matches the `serde(tag="tag", content="payload", rename_all="snake_case")`
        // on DiarizationCommand / DiarizationEvent in silent-core).
        let shaped = match outcome {
            RenameOutcome::Merge { from, target } => {
                serde_json::json!({ "tag": "merge", "payload": { "from": from, "target": target } })
            }
            RenameOutcome::Rename { id, name } => {
                serde_json::json!({ "tag": "rename", "payload": { "id": id, "name": name } })
            }
        };
        Ok(JsValue::from_str(&shaped.to_string()))
    }

    /// Apply a merge (the user confirmed the merge-by-rename prompt, or an
    /// explicit merge was requested). Folds `from_id` into `to_id`.
    ///
    /// Returns a JSON `{ "from_id": "S2", "to_id": "S1" }` on success, or
    /// `null` if the merge was a no-op (self-merge or unknown id).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn confirm_merge(&mut self, from_id: &str, to_id: &str) -> Result<JsValue, JsError> {
        if self.tracker.merge(from_id, to_id) {
            let v = serde_json::json!({ "from_id": from_id, "to_id": to_id });
            Ok(JsValue::from_str(&v.to_string()))
        } else {
            Ok(JsValue::null())
        }
    }

    /// Apply a plain rename to a speaker. No return value needed (the UI
    /// updates the DOM itself; this keeps the tracker in sync).
    pub fn rename(&mut self, speaker_id: &str, new_name: &str) {
        self.tracker.rename(speaker_id, new_name);
    }

    /// Run the stop-time global recluster (DIARIZATION.md §2, Appendix A row 15).
    ///
    /// `threshold` is the cosine similarity above which two clusters merge.
    /// Pass `NaN` to use the configured default (0.65).
    ///
    /// Returns a JSON object:
    /// ```json
    /// {
    ///   "relabel": [{ "old_id": "S5", "new_id": "S2" }, ...],
    ///   "speakers": [{ "id": "S1", "name": "Alice", "color": "#00d4aa", "count": 7 }, ...]
    /// }
    /// ```
    ///
    /// An empty `relabel` array means no merges were needed.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn global_recluster(&mut self, threshold: f32) -> Result<JsValue, JsError> {
        let th = if threshold.is_nan() {
            None
        } else {
            Some(threshold)
        };
        let relabel_map = self.tracker.global_recluster(th);
        let relabel: Vec<serde_json::Value> = relabel_map
            .iter()
            .map(|(old, new)| serde_json::json!({ "old_id": old, "new_id": new }))
            .collect();
        let speakers: Vec<serde_json::Value> = self
            .tracker
            .speakers()
            .iter()
            .map(speaker_to_json)
            .collect();
        let v = serde_json::json!({ "relabel": relabel, "speakers": speakers });
        Ok(JsValue::from_str(&v.to_string()))
    }

    /// Current snapshot of all speaker clusters, as a JSON array of
    /// `SpeakerDescriptor`. Used to rebuild the speakers bar (e.g. after
    /// `global_recluster`).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` only on JSON serialization failure.
    pub fn speakers(&self) -> Result<JsValue, JsError> {
        let speakers: Vec<serde_json::Value> = self
            .tracker
            .speakers()
            .iter()
            .map(speaker_to_json)
            .collect();
        let v = serde_json::json!(speakers);
        Ok(JsValue::from_str(&v.to_string()))
    }

    /// Reset the speaker tracker for a new meeting while keeping the loaded
    /// ONNX session alive (mirrors the `sharedSpeakerEmbedder` model-survive-
    /// meeting-reset semantics — the model stays loaded, the cluster state is
    /// cleared). Called from the JS `DiarizationEngine.reset()` method on new
    /// meeting.
    pub fn reset_tracker(&mut self) {
        self.tracker = SpeakerTracker::new(TrackerConfig::default());
    }
}

// ---------------------------------------------------------------------------
// Internal serde shapes (never escape the wasm boundary as raw types)
// ---------------------------------------------------------------------------

/// The JSON shape returned by `identify` / `reuse_last_speaker`. Matches the
/// JS `{ id, name, color, isNew }` shape (note: camelCase in JSON,
/// `snake_case` in Rust — `rename_all = "camelCase"` bridges them).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentifiedResult {
    id: String,
    name: String,
    color: String,
    is_new: bool,
}

/// Serialize a `Speaker` to the JSON shape the UI renders: id, name, color,
/// count. The centroid is intentionally omitted (raw embeddings never cross the
/// boundary — PRD R5/R7).
fn speaker_to_json(s: &Speaker) -> serde_json::Value {
    serde_json::json!({
        "id":    s.id,
        "name":  s.name,
        "color": s.color,
        "count": s.count,
    })
}
