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
//! that on the browser main thread -- there is no blocking `block_on` there.
//!
//! So the web path implements the *same* cache-aware chunking + greedy RNN-T
//! decode as [`crate::streaming::StreamingAsr::transcribe_audio`], in `async`
//! form, while reusing the unchanged [`crate::audio::MelFrontend`] front-end and
//! [`crate::vocab::SentencePieceVocab`] detokenizer. The mel-chunk construction
//! and `argmax` are delegated to [`crate::chunk_core`] -- shared with the native
//! path so the math (mel chunk layout, encoder length, cache carry-forward, blank
//! handling, state updates) cannot silently diverge.

use ndarray::{Array1, Array3, Array4};
use ort::session::{RunOptions, Session};
use ort::value::{Tensor, TensorRef};
use ort_web::{sync_outputs, FEATURE_NONE};
use wasm_bindgen::prelude::*;

use crate::audio::MelFrontend;
use crate::chunk_core::{
    argmax, build_offline_mel_chunk, build_streaming_mel_chunk, build_tail_mel_chunk,
};
use crate::constants::{
    BLANK_ID, CHUNK_SIZE, CONV_CONTEXT, DECODER_LSTM_DIM, EDGE_GUARD_FRAMES, HIDDEN_DIM,
    HOP_LENGTH, LEFT_CONTEXT, LSTM_LAYERS, MAX_SYMBOLS_PER_STEP, NUM_ENCODER_LAYERS, N_MELS,
    PRE_ENCODE_CACHE, VOCAB_SIZE, WIN_LENGTH,
};
use crate::vocab::SentencePieceVocab;

/// Initialise the `ort-web` backend (fetches the onnxruntime-web JS + WASM).
///
/// Must be called, and awaited, exactly once before constructing a
/// [`WasmAsr`]. Uses the CPU build (`FEATURE_NONE`) -- `FINDINGS.md` shows this
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

/// Mutable per-utterance decoder state, mirroring `streaming::DecodeState`,
/// plus the raw-audio buffering state needed for seam-free live streaming
/// (mirrors `parakeet-rs` `Nemotron`'s `audio_buffer` / `audio_processed` /
/// `chunk_idx`).
struct WebState {
    // --- encoder / decoder state (carried across chunks) ---
    cache_last_channel: Array4<f32>,
    cache_last_time: Array4<f32>,
    cache_last_channel_len: Array1<i64>,
    state_1: Array3<f32>,
    state_2: Array3<f32>,
    last_token: i32,

    // --- live-streaming raw-audio buffer state ---
    /// Raw 16 kHz mono samples retained for mel recomputation. Trimmed from the
    /// front as audio is processed to keep memory bounded.
    audio_buffer: Vec<f32>,
    /// How many raw samples have been consumed into encoder chunks so far.
    /// `audio_processed / HOP_LENGTH` is the next unprocessed mel frame.
    audio_processed: usize,
    /// Number of streaming chunks emitted so far (drives first-chunk lookback).
    chunk_idx: usize,
}

impl WebState {
    fn fresh() -> Self {
        // BLANK_ID = 1024, fits safely in i32.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        Self {
            cache_last_channel: Array4::zeros((NUM_ENCODER_LAYERS, 1, LEFT_CONTEXT, HIDDEN_DIM)),
            cache_last_time: Array4::zeros((NUM_ENCODER_LAYERS, 1, HIDDEN_DIM, CONV_CONTEXT)),
            cache_last_channel_len: Array1::from_vec(vec![0i64]),
            state_1: Array3::zeros((LSTM_LAYERS, 1, DECODER_LSTM_DIM)),
            state_2: Array3::zeros((LSTM_LAYERS, 1, DECODER_LSTM_DIM)),
            last_token: BLANK_ID as i32,
            audio_buffer: Vec::new(),
            audio_processed: 0,
            chunk_idx: 0,
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
    ///
    /// # Errors
    ///
    /// Returns `JsError` if `ort-web` initialisation fails, if either ONNX
    /// session cannot be built from the provided bytes, or if the tokenizer
    /// cannot be parsed.
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
    ///
    /// # Errors
    ///
    /// Returns `JsError` if the mel frontend, encoder, or decoder fails.
    pub async fn transcribe(&mut self, samples: &[f32]) -> Result<String, JsError> {
        self.reset();
        let tokens = self.run_over_audio(samples).await?;
        let valid: Vec<usize> = tokens.into_iter().filter(|&t| t < VOCAB_SIZE).collect();
        Ok(self.vocab.decode(&valid))
    }

    /// Transcribe an audio chunk incrementally for real-time use -- call
    /// repeatedly with successive slices of mic audio.
    ///
    /// Unlike a naive per-call mel, this **buffers raw audio and recomputes the
    /// mel over the whole retained buffer**, so each encoder chunk gets its real
    /// 9-frame pre-encode lookback and there is no `n_fft/2` zero-padding seam
    /// at chunk boundaries. This fixes the boundary degradation seen when
    /// feeding fixed chunks (e.g. "lazy" -> "laser" at 1 s seams). Mirrors the
    /// validated `parakeet-rs` `Nemotron::transcribe_chunk`.
    ///
    /// The incoming `samples` need not align to encoder-chunk boundaries; this
    /// consumes as many whole 56-frame chunks as the buffer now allows (the
    /// reference processes at most one per call -- we drain all available so a
    /// large incoming chunk, e.g. 1 s = ~100 frames, doesn't lag behind).
    /// Returns the text decoded during this call (may be empty if not enough
    /// new audio has accumulated yet).
    ///
    /// # Errors
    ///
    /// Returns `JsError` if the mel frontend, encoder, or decoder fails.
    pub async fn transcribe_chunk(&mut self, samples: &[f32]) -> Result<String, JsError> {
        let tokens = self.stream_step(samples).await?;
        let mut out = String::new();
        for &t in &tokens {
            if t < VOCAB_SIZE {
                out.push_str(&self.vocab.decode_single(t));
            }
        }
        Ok(out)
    }

    /// Flush the trailing partial chunk at end of stream.
    ///
    /// `transcribe_chunk` only consumes whole 56-frame chunks, so up to the last
    /// `< CHUNK_SIZE` mel frames (< 560 ms of audio) are left unprocessed. Call
    /// this once after the final `transcribe_chunk` to decode that remainder as
    /// one final partial chunk -- mirroring the offline `transcribe_audio`'s last
    /// iteration where `main_len < CHUNK_SIZE`. The carried decode state (encoder
    /// cache, LSTM state, `last_token`) is used as-is; this does **not** reset.
    /// Returns the text decoded from the tail (empty if nothing remained).
    ///
    /// # Errors
    ///
    /// Returns `JsError` if the mel frontend, encoder, or decoder fails.
    pub async fn finalize(&mut self) -> Result<String, JsError> {
        let tokens = self.flush_tail().await?;
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
    /// Live-streaming driver: append `samples` to the raw-audio buffer, then
    /// consume every whole 56-frame chunk now available, recomputing the mel
    /// over the entire retained buffer each pass so chunk boundaries keep real
    /// lookback and no zero-padding seam. Mirrors the reference
    /// `Nemotron::transcribe_chunk` math (buffer / processed / `chunk_idx` /
    /// trim), but loops over all newly-available chunks instead of one per call.
    ///
    /// Mel-chunk construction is delegated to [`build_streaming_mel_chunk`]
    /// from [`crate::chunk_core`].
    async fn stream_step(&mut self, samples: &[f32]) -> Result<Vec<usize>, JsError> {
        self.state.audio_buffer.extend_from_slice(samples);

        // Need at least one analysis window before any mel frame exists.
        if self.state.audio_buffer.len() < WIN_LENGTH {
            return Ok(Vec::new());
        }

        let mut emitted: Vec<usize> = Vec::new();

        loop {
            // Recompute the mel over the WHOLE retained buffer (this is the fix:
            // the lookback frames and the seam come from real, continuous audio
            // rather than a freshly zero-padded per-call mel).
            let full_mel = self
                .frontend
                .log_mel(&self.state.audio_buffer)
                .map_err(to_js)?;
            let total_mel_frames = full_mel.shape()[1];

            // Next unprocessed mel frame.
            let processed_mel_frames = self.state.audio_processed / HOP_LENGTH;

            // Stop once fewer than a full main chunk of CLEAN new frames
            // remains. The guard keeps the chunk's tail clear of the mel's
            // right-edge zero-padding zone -- frames there are computed against
            // synthetic zeros rather than the audio that arrives next, which
            // corrupts both the decode and the carried encoder cache (see
            // `EDGE_GUARD_FRAMES`). Those frames are re-derived cleanly on a
            // later call once their real audio exists; `flush_tail` consumes
            // the genuine end-of-stream remainder without the guard.
            let available_new_frames = total_mel_frames.saturating_sub(processed_mel_frames);
            if available_new_frames < CHUNK_SIZE + EDGE_GUARD_FRAMES {
                break;
            }

            let main_start = processed_mel_frames;
            let (chunk_data, chunk_size) = build_streaming_mel_chunk(
                &full_mel,
                self.state.chunk_idx,
                main_start,
                total_mel_frames,
            );

            // A streaming chunk always carries a full 56 main frames; encoder
            // length is the full expected window. chunk_size <= 65, fits in i64.
            #[allow(clippy::cast_possible_wrap)]
            let (encoded, enc_len) = self
                .run_encoder(&chunk_data, chunk_size, chunk_size as i64)
                .await?;
            let tokens = self.decode_chunk(&encoded, enc_len).await?;
            emitted.extend(tokens);

            // Advance by exactly one main chunk of audio.
            self.state.audio_processed += CHUNK_SIZE * HOP_LENGTH;
            self.state.chunk_idx += 1;

            // Trim the front of the buffer to bound memory, keeping enough tail
            // for the next chunk's lookback + window. Decrement `audio_processed`
            // by the same sample count so frame indices stay consistent against
            // the recomputed mel. (Mirrors the reference trim exactly.)
            let keep_samples = (PRE_ENCODE_CACHE + CHUNK_SIZE) * HOP_LENGTH + WIN_LENGTH;
            if self.state.audio_buffer.len() > keep_samples * 2 {
                let remove = self.state.audio_buffer.len() - keep_samples;
                let actual_remove = remove.min(self.state.audio_processed);
                self.state.audio_buffer.drain(0..actual_remove);
                self.state.audio_processed -= actual_remove;
            }
        }

        Ok(emitted)
    }

    /// Decode the leftover `< CHUNK_SIZE` mel frames that `stream_step` left
    /// unconsumed, as one final partial chunk. Mirrors the final iteration of
    /// the offline `transcribe_audio` (`main_len < CHUNK_SIZE`) and the
    /// streaming path's real pre-encode lookback. Carries decode state forward.
    ///
    /// Mel-chunk construction is delegated to [`build_tail_mel_chunk`]
    /// from [`crate::chunk_core`].
    async fn flush_tail(&mut self) -> Result<Vec<usize>, JsError> {
        // Without at least one analysis window there are no mel frames to flush.
        if self.state.audio_buffer.len() < WIN_LENGTH {
            return Ok(Vec::new());
        }

        let full_mel = self
            .frontend
            .log_mel(&self.state.audio_buffer)
            .map_err(to_js)?;
        let total_mel_frames = full_mel.shape()[1];

        let main_start = self.state.audio_processed / HOP_LENGTH;
        let available = total_mel_frames.saturating_sub(main_start);
        if available == 0 {
            return Ok(Vec::new());
        }

        let (chunk_data, chunk_length) = build_tail_mel_chunk(&full_mel, main_start, available);

        let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
        let (encoded, enc_len) = self
            .run_encoder(&chunk_data, expected_size, chunk_length)
            .await?;
        let tokens = self.decode_chunk(&encoded, enc_len).await?;

        // Advance so a redundant second finalize is a no-op.
        self.state.audio_processed += available * HOP_LENGTH;
        self.state.chunk_idx += 1;

        Ok(tokens)
    }

    /// Core loop shared by `transcribe` (offline): compute the
    /// log-mel of `audio`, slide 56-frame chunks, run the encoder, and greedily
    /// decode. Mirrors `streaming::StreamingAsr::transcribe_audio` exactly,
    /// minus the leading `reset()` (callers decide).
    ///
    /// Mel-chunk construction is delegated to [`build_offline_mel_chunk`]
    /// from [`crate::chunk_core`] -- shared with the native offline path.
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

            let chunk_data = build_offline_mel_chunk(&mel, chunk_idx, buffer_idx, main_len);

            // PRE_ENCODE_CACHE + main_len <= 65, fits in i64.
            #[allow(clippy::cast_possible_wrap)]
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
        // N_MELS = 128, expected_size <= 65 -- both fit safely in i64.
        #[allow(clippy::cast_possible_wrap)]
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
        let enc_len = extract_i64_scalar(&outputs, "encoded_len")?;

        self.state.cache_last_channel = extract4(&outputs, "cache_last_channel_next")?;
        self.state.cache_last_time = extract4(&outputs, "cache_last_time_next")?;
        self.state.cache_last_channel_len = Array1::from_vec(vec![extract_i64_scalar(
            &outputs,
            "cache_last_channel_len_next",
        )?]);

        // enc_len is the encoder's reported frame count -- always non-negative
        // and <= CHUNK_SIZE (56); safe to cast from i64 to usize.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let enc_frames = enc_len as usize;
        Ok((encoded, enc_frames))
    }

    /// Greedy RNN-T decode of `enc_frames` encoder frames. Mirrors
    /// `streaming::StreamingAsr::decode_chunk` (up to 10 symbols/frame, blank
    /// breaks the frame, state advances only on non-blank tokens).
    ///
    /// `argmax` is delegated to [`crate::chunk_core::argmax`] -- shared with
    /// the native path.
    async fn decode_chunk(
        &mut self,
        encoded: &Array3<f32>,
        enc_frames: usize,
    ) -> Result<Vec<usize>, JsError> {
        let mut tokens = Vec::new();
        let hidden = encoded.shape()[1];

        for t in 0..enc_frames {
            // Encoder frame [1, HIDDEN_DIM, 1].
            // `index_axis` avoids the `s![]` macro whose expansion contains
            // `#[allow(unsafe_code)]` (ndarray internal), incompatible with
            // `workspace.lints.rust.unsafe_code = "forbid"` (E0453).
            let frame_owned = encoded
                .index_axis(ndarray::Axis(0), 0)
                .index_axis(ndarray::Axis(1), t)
                .to_owned();
            let frame_vec = frame_owned.to_vec();

            for _ in 0..MAX_SYMBOLS_PER_STEP {
                // HIDDEN_DIM = 1024, fits safely in i64.
                #[allow(clippy::cast_possible_wrap)]
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
                // max_idx <= VOCAB_SIZE = 1024, fits in i32.
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                {
                    self.state.last_token = max_idx as i32;
                }
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
///
/// LSTM state dimensions are architecture constants (O(hundreds)); they
/// always fit in i64 -- `usize as i64` wrapping is physically impossible.
#[allow(clippy::cast_possible_wrap)] // state dims are O(100), fit safely in i64
fn tensor3(a: &Array3<f32>) -> Result<Tensor<f32>, JsError> {
    let dims = a.shape();
    let shape = [dims[0] as i64, dims[1] as i64, dims[2] as i64];
    let data = a.iter().copied().collect::<Vec<f32>>();
    Tensor::from_array((shape, data)).map_err(to_js)
}

/// Build an owned `Tensor<f32>` from a 4-D ndarray (encoder caches).
///
/// Cache dimensions are architecture constants (O(thousands)); they always
/// fit in i64 -- `usize as i64` wrapping is physically impossible.
#[allow(clippy::cast_possible_wrap)] // cache dims are O(1000), fit safely in i64
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

/// ONNX tensor shapes are `i64`; we cast to `usize`. The model's architecture
/// constants (`HIDDEN_DIM`, `LEFT_CONTEXT`, etc.) are always non-negative and
/// fit in a `usize` -- a shape dim of >= 2^63 is physically impossible.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn extract3(outputs: &ort::session::SessionOutputs, name: &str) -> Result<Array3<f32>, JsError> {
    let (shape, data) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(|e| JsError::new(&format!("extract `{name}`: {e}")))?;
    let d: Vec<usize> = shape.as_ref().iter().map(|&x| x as usize).collect();
    Array3::from_shape_vec((d[0], d[1], d[2]), data.to_vec())
        .map_err(|e| JsError::new(&format!("reshape `{name}`: {e}")))
}

/// Same cast rationale as `extract3`.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
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
