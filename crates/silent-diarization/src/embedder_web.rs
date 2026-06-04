//! Browser wasm32 TitaNet-small embedder using `ort-web` (onnxruntime-web).
//!
//! Productionized from spike b1 (`docs/research/spike-titanet.md`): the browser
//! leg matched the native leg at cosine **1.000000** (worst 0.99999988, f32
//! rounding in the cosine reduction) with `crossOriginIsolated === true` and
//! **zero** fetches to `cdn.pyke.io` when the ort-web runtime is vendored
//! same-origin.
//!
//! The front-end is the identical [`silent_audio::MelFrontend`]
//! ([`silent_audio::titanet_config`]) the native path uses — same code, run
//! under ort-web's WASM ORT kernel. The only differences from native are async
//! session creation from in-memory bytes and async tensor I/O.
//!
//! Same-origin vendored assets (sidestepping the COEP/SharedArrayBuffer problem)
//! are loaded via [`WasmTitaNetEmbedder::create_with_dist`]; the default CDN via
//! [`WasmTitaNetEmbedder::create`].

// The ONNX tensor shape dims (mel bands, frame count) are cast usize→i64 for the
// ort-web `Tensor::from_array` shape, matching the API. These are tiny model
// dimensions that cannot wrap an i64. (This file only compiles on wasm32, so the
// lesson from d3 applies: clippy this target, not just native.)
#![allow(
    clippy::cast_possible_wrap,
    reason = "ONNX tensor shape dims (mel bands, frame count) are small model \
              dimensions that cannot wrap an i64"
)]

use silent_audio::MelFrontend;

use ort::session::{RunOptions, Session};
use ort::value::Tensor;
use ort_web::{Dist, FEATURE_NONE, sync_outputs};
use wasm_bindgen::prelude::*;

/// Initialise the `ort-web` backend from the default CDN (`cdn.pyke.io`).
async fn init_ort_web_default() -> std::result::Result<(), JsError> {
    let api = ort_web::api(FEATURE_NONE)
        .await
        .map_err(|e| JsError::new(&format!("ort-web init failed: {e}")))?;
    ort::set_api(api);
    let _ = ort::init().with_telemetry(false).commit();
    Ok(())
}

/// Initialise the `ort-web` backend from a custom same-origin base URL. This is
/// the path that sidesteps the COEP problem (B3's recommendation): serve the
/// ort-web assets from the SAME origin as the wasm bundle so COEP never blocks
/// them and `crossOriginIsolated` stays true.
async fn init_ort_web_vendored(base_url: &str) -> std::result::Result<(), JsError> {
    let dist = Dist::new(base_url).with_binary_name("ort-wasm-simd-threaded.wasm");
    let api = ort_web::api(dist)
        .await
        .map_err(|e| JsError::new(&format!("ort-web init (vendored {base_url}) failed: {e}")))?;
    ort::set_api(api);
    let _ = ort::init().with_telemetry(false).commit();
    Ok(())
}

fn to_js<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Browser-facing TitaNet-small speaker embedder.
///
/// Feed 16 kHz mono f32 audio via [`WasmTitaNetEmbedder::embed`]; receive a
/// 192-d L2-normalized embedding (a flat `Float32Array` in JS).
#[wasm_bindgen]
pub struct WasmTitaNetEmbedder {
    session: Session,
    frontend: MelFrontend,
    run_opts: RunOptions,
}

#[wasm_bindgen]
impl WasmTitaNetEmbedder {
    /// Create an embedder, loading the ort-web runtime from the default CDN.
    ///
    /// - `onnx_bytes`: bytes of `titanet.onnx` (fetched via the registry pin).
    /// - `mel_fb_json`: UTF-8 JSON bytes of `mel_fb.json` (the 80×257 slaney
    ///   matrix), also fetched via the registry pin.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the ort-web runtime, the ONNX session, or the mel
    /// filterbank cannot be initialised.
    pub async fn create(
        onnx_bytes: &[u8],
        mel_fb_json: &[u8],
    ) -> std::result::Result<WasmTitaNetEmbedder, JsError> {
        console_error_panic_hook::set_once();
        init_ort_web_default().await?;
        Self::build(onnx_bytes, mel_fb_json).await
    }

    /// Create an embedder, loading the ort-web runtime from a same-origin
    /// vendored base URL (e.g. `"./vendor/"`). The path that keeps
    /// `crossOriginIsolated === true` (B3 vendoring).
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the vendored ort-web runtime, the ONNX session, or
    /// the mel filterbank cannot be initialised.
    pub async fn create_with_dist(
        onnx_bytes: &[u8],
        mel_fb_json: &[u8],
        dist_base_url: &str,
    ) -> std::result::Result<WasmTitaNetEmbedder, JsError> {
        console_error_panic_hook::set_once();
        init_ort_web_vendored(dist_base_url).await?;
        Self::build(onnx_bytes, mel_fb_json).await
    }

    /// Shared construction (runtime must already be initialised).
    async fn build(
        onnx_bytes: &[u8],
        mel_fb_json: &[u8],
    ) -> std::result::Result<WasmTitaNetEmbedder, JsError> {
        let session = Session::builder()
            .map_err(to_js)?
            .commit_from_memory(onnx_bytes)
            .await
            .map_err(|e| JsError::new(&format!("failed to load titanet.onnx: {e}")))?;

        let frontend = MelFrontend::titanet_from_mel_fb_json(mel_fb_json)
            .map_err(|e| JsError::new(&e.to_string()))?;

        Ok(WasmTitaNetEmbedder {
            session,
            frontend,
            run_opts: RunOptions::new().map_err(to_js)?,
        })
    }

    /// Compute the 192-d L2-normalized speaker embedding for `samples`
    /// (16 kHz mono f32). Returns a `Float32Array` of length 192.
    ///
    /// # Errors
    ///
    /// Returns a `JsError` if the mel front-end or ONNX inference fails.
    pub async fn embed(&mut self, samples: &[f32]) -> std::result::Result<Vec<f32>, JsError> {
        let mel = self
            .frontend
            .log_mel(samples)
            .map_err(|e| JsError::new(&e.to_string()))?;
        let n_mels = mel.shape()[0];
        let t = mel.shape()[1];

        let audio_data = mel.into_raw_vec_and_offset().0;
        let audio_tensor =
            Tensor::from_array(([1_i64, n_mels as i64, t as i64], audio_data)).map_err(to_js)?;
        let length_tensor = Tensor::from_array(([1_i64], vec![t as i64])).map_err(to_js)?;

        let mut outputs = self
            .session
            .run_async(
                ort::inputs![
                    "audio_signal" => audio_tensor,
                    "length" => length_tensor,
                ],
                &self.run_opts,
            )
            .await
            .map_err(|e| JsError::new(&format!("titanet run failed: {e}")))?;

        sync_outputs(&mut outputs)
            .await
            .map_err(|e| JsError::new(&format!("titanet output sync failed: {e}")))?;

        let (_, emb_data) = outputs["embs"]
            .try_extract_tensor::<f32>()
            .map_err(|e| JsError::new(&format!("extract embs: {e}")))?;

        Ok(emb_data.to_vec())
    }
}

/// Cosine similarity helper exposed to JS for validation (matches the spike's
/// browser-leg cosine check).
#[wasm_bindgen]
#[must_use]
pub fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}
