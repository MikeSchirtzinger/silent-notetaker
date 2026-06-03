//! wasm32 / browser backend over `ort-web` (onnxruntime-web).
//!
//! This module is compiled **only** for `target_arch = "wasm32"`. The native
//! build, the [`crate::model::AsrBackend`] trait, and `audio.rs` / `vocab.rs` /
//! `streaming.rs` are all untouched.
//!
//! ## Why this isn't an `AsrBackend` impl
//!
//! `ort-web` is **async-only**: `Session::run_async(...).await`, async session
//! creation, and outputs must be `.sync(SyncDirection::Rust).await`-ed before
//! their data is readable from Rust. The synchronous [`crate::model::AsrBackend`]
//! trait (and the tight synchronous loops in [`crate::streaming`]) cannot drive
//! that on the browser main thread — there is no blocking `block_on` there.
//!
//! So the web path re-implements the *same* cache-aware chunking + greedy RNN-T
//! decode as [`crate::streaming::StreamingAsr::transcribe_audio`], in `async`
//! form, while reusing the unchanged [`crate::audio::MelFrontend`] front-end and
//! [`crate::vocab::SentencePieceVocab`] detokenizer. The math (mel chunk layout,
//! encoder length, cache carry-forward, blank handling, state updates) mirrors
//! the validated native path exactly.

use ndarray::{s, Array1, Array3, Array4};
use ort::session::{RunOptions, Session};
use ort::value::{Tensor, TensorRef};
use ort_web::{sync_outputs, FEATURE_NONE};
use wasm_bindgen::prelude::*;

use crate::audio::MelFrontend;
use crate::constants::{
    BLANK_ID, CHUNK_SIZE, CONV_CONTEXT, DECODER_LSTM_DIM, HIDDEN_DIM, LEFT_CONTEXT, LSTM_LAYERS,
    MAX_SYMBOLS_PER_STEP, NUM_ENCODER_LAYERS, N_MELS, PRE_ENCODE_CACHE, VOCAB_SIZE,
};
use crate::vocab::SentencePieceVocab;

/// Initialise the `ort-web` backend (fetches the onnxruntime-web JS + WASM).
///
/// Must be called, and awaited, exactly once before constructing a
/// [`WasmAsr`]. Uses the CPU build (`FEATURE_NONE`) — `FINDINGS.md` shows this
/// model runs comfortably in realtime on CPU and WebGPU is actively worse for
/// this model class. By default `ort-web` fetches its assets from
/// `cdn.pyke.io`; ensure that origin is permitted by your Content-Security-Policy
/// `connect-src`/`script-src`.
async fn init_ort_web() -> Result<(), JsError> {
    let api = ort_web::api(FEATURE_NONE)
        .await
        .map_err(|e| JsError::new(&format!("ort-web init failed: {e}")))?;
    // Safe to call once; sets the global OrtApi used by all ort calls.
    ort::set_api(api);
    // Telemetry would otherwise phone home on first session commit.
    let _ = ort::init().with_telemetry(false).commit();
    Ok(())
}

/// Mutable per-utterance decoder state, mirroring `streaming::DecodeState`.
struct WebState {
    cache_last_channel: Array4<f32>,
    cache_last_time: Array4<f32>,
    cache_last_channel_len: Array1<i64>,
    state_1: Array3<f32>,
    state_2: Array3<f32>,
    last_token: i32,
}

impl WebState {
    fn fresh() -> Self {
        Self {
            cache_last_channel: Array4::zeros((NUM_ENCODER_LAYERS, 1, LEFT_CONTEXT, HIDDEN_DIM)),
            cache_last_time: Array4::zeros((NUM_ENCODER_LAYERS, 1, HIDDEN_DIM, CONV_CONTEXT)),
            cache_last_channel_len: Array1::from_vec(vec![0i64]),
            state_1: Array3::zeros((LSTM_LAYERS, 1, DECODER_LSTM_DIM)),
            state_2: Array3::zeros((LSTM_LAYERS, 1, DECODER_LSTM_DIM)),
            last_token: BLANK_ID as i32,
        }
    }
}

/// Browser-facing streaming ASR engine.
///
/// Construct with [`WasmAsr::create`], passing the three model artifacts as
/// byte buffers (the JS side fetches them). The encoder and FP32 decoder/joint
/// ONNX sessions live in the onnxruntime-web WASM context; this struct holds
/// the Rust-side mel front-end, tokenizer, and decode state.
#[wasm_bindgen]
pub struct WasmAsr {
    encoder: Session,
    decoder: Session,
    frontend: MelFrontend,
    vocab: SentencePieceVocab,
    state: WebState,
    run_opts: RunOptions,
}

#[wasm_bindgen]
impl WasmAsr {
    /// Create an engine from in-memory model bytes.
    ///
    /// - `encoder_onnx`: bytes of `encoder.onnx` (INT8).
    /// - `decoder_onnx`: bytes of `decoder_joint_fp32.onnx` (FP32 LSTM).
    /// - `tokenizer_model`: bytes of `tokenizer.model` (SentencePiece).
    ///
    /// This initialises `ort-web` (fetching onnxruntime-web) on first call and
    /// commits both ONNX sessions from memory. Both steps are async.
    pub async fn create(
        encoder_onnx: &[u8],
        decoder_onnx: &[u8],
        tokenizer_model: &[u8],
    ) -> Result<WasmAsr, JsError> {
        console_error_panic_hook::set_once();
        init_ort_web().await?;

        let encoder = Session::builder()
            .map_err(to_js)?
            .commit_from_memory(encoder_onnx)
            .await
            .map_err(|e| JsError::new(&format!("failed to load encoder: {e}")))?;
        let decoder = Session::builder()
            .map_err(to_js)?
            .commit_from_memory(decoder_onnx)
            .await
            .map_err(|e| JsError::new(&format!("failed to load decoder: {e}")))?;

        let frontend = MelFrontend::new();
        let vocab = SentencePieceVocab::from_bytes(tokenizer_model).map_err(to_js)?;

        Ok(WasmAsr {
            encoder,
            decoder,
            frontend,
            vocab,
            state: WebState::fresh(),
            run_opts: RunOptions::new().map_err(to_js)?,
        })
    }

    /// Reset all streaming state for a new utterance.
    pub fn reset(&mut self) {
        self.state = WebState::fresh();
    }

    /// Transcribe a full audio buffer offline (resets state first).
    ///
    /// `samples` must be 16 kHz mono `f32` in `[-1, 1]`. Returns the full
    /// detokenized transcript. This is the wasm analogue of the validated
    /// native `transcribe_audio`.
    pub async fn transcribe(&mut self, samples: &[f32]) -> Result<String, JsError> {
        self.reset();
        let tokens = self.run_over_audio(samples).await?;
        let valid: Vec<usize> = tokens.into_iter().filter(|&t| t < VOCAB_SIZE).collect();
        Ok(self.vocab.decode(&valid))
    }

    /// Transcribe an audio chunk incrementally, carrying state across calls.
    ///
    /// Note: like the native `decode_chunk`, this treats `samples` as a fresh
    /// mel computation feeding the carried encoder/decoder state. For the
    /// cleanest results, feed reasonably sized chunks (e.g. 560 ms+). Returns
    /// the text emitted for this chunk only.
    pub async fn transcribe_chunk(&mut self, samples: &[f32]) -> Result<String, JsError> {
        let tokens = self.run_over_audio(samples).await?;
        let mut out = String::new();
        for &t in &tokens {
            if t < VOCAB_SIZE {
                out.push_str(&self.vocab.decode_single(t));
            }
        }
        Ok(out)
    }
}

impl WasmAsr {
    /// Core loop shared by `transcribe` / `transcribe_chunk`: compute the
    /// log-mel of `audio`, slide 56-frame chunks, run the encoder, and greedily
    /// decode. Mirrors `streaming::StreamingAsr::transcribe_audio` exactly,
    /// minus the leading `reset()` (callers decide).
    async fn run_over_audio(&mut self, audio: &[f32]) -> Result<Vec<usize>, JsError> {
        let mel = self.frontend.log_mel(audio).map_err(to_js)?;
        let total_frames = mel.shape()[1];
        if total_frames == 0 {
            return Ok(Vec::new());
        }

        let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
        let mut all_tokens: Vec<usize> = Vec::new();
        let mut buffer_idx = 0usize;
        let mut chunk_idx = 0usize;

        while buffer_idx < total_frames {
            let chunk_end = (buffer_idx + CHUNK_SIZE).min(total_frames);
            let main_len = chunk_end - buffer_idx;

            // [N_MELS, expected_size] flattened mel-major == encoder [1,128,T].
            let mut chunk_data = vec![0.0f32; N_MELS * expected_size];

            if chunk_idx > 0 && buffer_idx >= PRE_ENCODE_CACHE {
                let cache_start = buffer_idx - PRE_ENCODE_CACHE;
                for f in 0..PRE_ENCODE_CACHE {
                    for m in 0..N_MELS {
                        chunk_data[m * expected_size + f] = mel[[m, cache_start + f]];
                    }
                }
            }
            for f in 0..main_len {
                for m in 0..N_MELS {
                    chunk_data[m * expected_size + PRE_ENCODE_CACHE + f] = mel[[m, buffer_idx + f]];
                }
            }

            let chunk_length = (PRE_ENCODE_CACHE + main_len) as i64;
            let (encoded, enc_len) = self
                .run_encoder(&chunk_data, expected_size, chunk_length)
                .await?;
            let tokens = self.decode_chunk(&encoded, enc_len).await?;
            all_tokens.extend(tokens);

            buffer_idx += CHUNK_SIZE;
            chunk_idx += 1;
        }

        Ok(all_tokens)
    }

    /// Run the encoder over one mel chunk, returning `(encoded[1,1024,T], T)`
    /// and updating the carried cache state.
    async fn run_encoder(
        &mut self,
        chunk_data: &[f32],
        expected_size: usize,
        length: i64,
    ) -> Result<(Array3<f32>, usize), JsError> {
        let processed_signal =
            TensorRef::from_array_view(([1i64, N_MELS as i64, expected_size as i64], chunk_data))
                .map_err(to_js)?;
        let length_arr = Tensor::from_array(([1i64], vec![length])).map_err(to_js)?;
        let cache_ch = tensor4(&self.state.cache_last_channel)?;
        let cache_tm = tensor4(&self.state.cache_last_time)?;
        let cache_len = Tensor::from_array(([1i64], self.state.cache_last_channel_len.to_vec()))
            .map_err(to_js)?;

        let mut outputs = self
            .encoder
            .run_async(
                ort::inputs![
                    "processed_signal" => processed_signal,
                    "processed_signal_length" => length_arr,
                    "cache_last_channel" => cache_ch,
                    "cache_last_time" => cache_tm,
                    "cache_last_channel_len" => cache_len,
                ],
                &self.run_opts,
            )
            .await
            .map_err(|e| JsError::new(&format!("encoder run failed: {e}")))?;

        sync_outputs(&mut outputs)
            .await
            .map_err(|e| JsError::new(&format!("encoder output sync failed: {e}")))?;

        let encoded = extract3(&outputs, "encoded")?;
        let enc_len = extract_i64_scalar(&outputs, "encoded_len")? as usize;

        self.state.cache_last_channel = extract4(&outputs, "cache_last_channel_next")?;
        self.state.cache_last_time = extract4(&outputs, "cache_last_time_next")?;
        self.state.cache_last_channel_len = Array1::from_vec(vec![extract_i64_scalar(
            &outputs,
            "cache_last_channel_len_next",
        )?]);

        Ok((encoded, enc_len))
    }

    /// Greedy RNN-T decode of `enc_frames` encoder frames. Mirrors
    /// `streaming::StreamingAsr::decode_chunk` (up to 10 symbols/frame, blank
    /// breaks the frame, state advances only on non-blank tokens).
    async fn decode_chunk(
        &mut self,
        encoded: &Array3<f32>,
        enc_frames: usize,
    ) -> Result<Vec<usize>, JsError> {
        let mut tokens = Vec::new();
        let hidden = encoded.shape()[1];

        for t in 0..enc_frames {
            // Encoder frame [1, HIDDEN_DIM, 1].
            let frame_owned = encoded.slice(s![0, .., t]).to_owned();
            let frame_vec = frame_owned.to_vec();

            for _ in 0..MAX_SYMBOLS_PER_STEP {
                let enc_frame =
                    TensorRef::from_array_view(([1i64, hidden as i64, 1i64], frame_vec.as_slice()))
                        .map_err(to_js)?;
                let targets = Tensor::from_array(([1i64, 1i64], vec![self.state.last_token]))
                    .map_err(to_js)?;
                let target_len = Tensor::from_array(([1i64], vec![1i32])).map_err(to_js)?;
                let in_s1 = tensor3(&self.state.state_1)?;
                let in_s2 = tensor3(&self.state.state_2)?;

                let mut outputs = self
                    .decoder
                    .run_async(
                        ort::inputs![
                            "encoder_outputs" => enc_frame,
                            "targets" => targets,
                            "target_length" => target_len,
                            "input_states_1" => in_s1,
                            "input_states_2" => in_s2,
                        ],
                        &self.run_opts,
                    )
                    .await
                    .map_err(|e| JsError::new(&format!("decoder run failed: {e}")))?;

                sync_outputs(&mut outputs)
                    .await
                    .map_err(|e| JsError::new(&format!("decoder output sync failed: {e}")))?;

                let logits = extract1(&outputs, "outputs")?;
                let max_idx = argmax(&logits);

                if max_idx == BLANK_ID {
                    break;
                }

                tokens.push(max_idx);
                self.state.last_token = max_idx as i32;
                self.state.state_1 = extract3(&outputs, "output_states_1")?;
                self.state.state_2 = extract3(&outputs, "output_states_2")?;
            }
        }

        Ok(tokens)
    }
}

// --- small ort-web marshalling helpers ---

fn to_js<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

/// Build an owned `Tensor<f32>` from a 3-D ndarray (states).
fn tensor3(a: &Array3<f32>) -> Result<Tensor<f32>, JsError> {
    let dims = a.shape();
    let shape = [dims[0] as i64, dims[1] as i64, dims[2] as i64];
    let data = a.iter().copied().collect::<Vec<f32>>();
    Tensor::from_array((shape, data)).map_err(to_js)
}

/// Build an owned `Tensor<f32>` from a 4-D ndarray (encoder caches).
fn tensor4(a: &Array4<f32>) -> Result<Tensor<f32>, JsError> {
    let dims = a.shape();
    let shape = [
        dims[0] as i64,
        dims[1] as i64,
        dims[2] as i64,
        dims[3] as i64,
    ];
    let data = a.iter().copied().collect::<Vec<f32>>();
    Tensor::from_array((shape, data)).map_err(to_js)
}

fn extract1(outputs: &ort::session::SessionOutputs, name: &str) -> Result<Vec<f32>, JsError> {
    let (_, data) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(|e| JsError::new(&format!("extract `{name}`: {e}")))?;
    Ok(data.to_vec())
}

fn extract3(outputs: &ort::session::SessionOutputs, name: &str) -> Result<Array3<f32>, JsError> {
    let (shape, data) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(|e| JsError::new(&format!("extract `{name}`: {e}")))?;
    let d: Vec<usize> = shape.as_ref().iter().map(|&x| x as usize).collect();
    Array3::from_shape_vec((d[0], d[1], d[2]), data.to_vec())
        .map_err(|e| JsError::new(&format!("reshape `{name}`: {e}")))
}

fn extract4(outputs: &ort::session::SessionOutputs, name: &str) -> Result<Array4<f32>, JsError> {
    let (shape, data) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(|e| JsError::new(&format!("extract `{name}`: {e}")))?;
    let d: Vec<usize> = shape.as_ref().iter().map(|&x| x as usize).collect();
    Array4::from_shape_vec((d[0], d[1], d[2], d[3]), data.to_vec())
        .map_err(|e| JsError::new(&format!("reshape `{name}`: {e}")))
}

fn extract_i64_scalar(outputs: &ort::session::SessionOutputs, name: &str) -> Result<i64, JsError> {
    let (_, data) = outputs[name]
        .try_extract_tensor::<i64>()
        .map_err(|e| JsError::new(&format!("extract `{name}`: {e}")))?;
    data.first()
        .copied()
        .ok_or_else(|| JsError::new(&format!("`{name}` was empty")))
}

/// `np.argmax` (first max on ties), matching the native path.
fn argmax(values: &[f32]) -> usize {
    let mut max_idx = 0;
    let mut max_val = f32::NEG_INFINITY;
    for (i, &v) in values.iter().enumerate() {
        if v > max_val {
            max_val = v;
            max_idx = i;
        }
    }
    max_idx
}
