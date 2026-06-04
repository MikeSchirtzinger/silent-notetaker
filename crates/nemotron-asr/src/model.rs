//! ONNX session wrapper for the Nemotron encoder + decoder/joint network.
//!
//! All ONNX Runtime interaction lives behind the [`AsrBackend`] trait so a
//! future `wasm32` / `ort-web` implementation can be swapped in without
//! touching [`crate::streaming`]. The native implementation, [`OrtBackend`],
//! is the only thing that links `ort`; everything above it speaks in
//! `ndarray` arrays and the plain data types defined here.

use ndarray::{Array1, Array3, Array4};

use crate::constants::{CONV_CONTEXT, HIDDEN_DIM, LEFT_CONTEXT, NUM_ENCODER_LAYERS};
use crate::error::Result;

/// Cache-aware encoder state carried between streaming chunks.
///
/// All three tensors are produced by the encoder as `*_next` outputs and fed
/// straight back in on the following chunk.
#[derive(Clone)]
pub struct EncoderCache {
    /// `[NUM_ENCODER_LAYERS, 1, LEFT_CONTEXT, HIDDEN_DIM]`.
    pub last_channel: Array4<f32>,
    /// `[NUM_ENCODER_LAYERS, 1, HIDDEN_DIM, CONV_CONTEXT]`.
    pub last_time: Array4<f32>,
    /// `[1]` -- number of valid cached frames so far.
    pub last_channel_len: Array1<i64>,
}

impl Default for EncoderCache {
    fn default() -> Self {
        Self::new()
    }
}

impl EncoderCache {
    /// A zeroed cache with `last_channel_len = 0`, i.e. the start of an
    /// utterance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_channel: Array4::zeros((NUM_ENCODER_LAYERS, 1, LEFT_CONTEXT, HIDDEN_DIM)),
            last_time: Array4::zeros((NUM_ENCODER_LAYERS, 1, HIDDEN_DIM, CONV_CONTEXT)),
            last_channel_len: Array1::from_vec(vec![0i64]),
        }
    }
}

/// Output of one encoder forward pass.
pub struct EncoderOutput {
    /// Encoded features `[1, HIDDEN_DIM, T_enc]`.
    pub encoded: Array3<f32>,
    /// Number of valid encoder frames in `encoded`.
    pub encoded_len: i64,
    /// Updated cache to carry into the next chunk.
    pub cache: EncoderCache,
}

/// Output of one decoder/joint step.
pub struct DecoderOutput {
    /// Joint logits, flattened to length `VOCAB_SIZE + 1` (1025).
    pub logits: Array1<f32>,
    /// Updated LSTM state 1 `[LSTM_LAYERS, 1, DECODER_LSTM_DIM]`.
    pub state_1: Array3<f32>,
    /// Updated LSTM state 2 `[LSTM_LAYERS, 1, DECODER_LSTM_DIM]`.
    pub state_2: Array3<f32>,
}

/// The inference boundary the streaming engine talks to.
///
/// Implemented natively by [`OrtBackend`]. A `wasm32` backend over
/// `onnxruntime-web` would implement this same trait and be selected via a
/// `cfg`-gated type alias in [`crate::streaming`].
pub trait AsrBackend {
    /// Run the encoder over one mel chunk `[1, N_MELS, T]` with `length` valid
    /// frames and the given cache state.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Model`] if the ONNX session run fails or
    /// the output tensors cannot be extracted.
    fn run_encoder(
        &mut self,
        features: &Array3<f32>,
        length: i64,
        cache: &EncoderCache,
    ) -> Result<EncoderOutput>;

    /// Run one decoder/joint step for a single encoder frame
    /// `[1, HIDDEN_DIM, 1]` and the current `(target_token, state_1, state_2)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Model`] if the ONNX session run fails or
    /// the output tensors cannot be extracted.
    fn run_decoder(
        &mut self,
        encoder_frame: &Array3<f32>,
        target_token: i32,
        state_1: &Array3<f32>,
        state_2: &Array3<f32>,
    ) -> Result<DecoderOutput>;
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::OrtBackend;

/// Native ONNX Runtime backend (the only module that links `ort`).
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use ndarray::{Array1, Array2, Array3, Array4};
    use ort::session::Session;
    use ort::value::Value;
    use std::path::Path;

    use crate::error::{Error, Result};
    use crate::model::{AsrBackend, DecoderOutput, EncoderCache, EncoderOutput};

    /// Native backend holding the encoder and decoder/joint ONNX sessions.
    pub struct OrtBackend {
        encoder: Session,
        decoder_joint: Session,
    }

    impl OrtBackend {
        /// Load `encoder.onnx` and the FP32 decoder/joint from `model_dir`.
        ///
        /// We use `decoder_joint_fp32.onnx` (pure `ai.onnx` opset-17, standard
        /// `LSTM`) rather than the INT8 `decoder_joint.onnx`, which relies on
        /// the `com.microsoft.DynamicQuantizeLSTM` contrib op that is not
        /// guaranteed to exist in the future `ort-web` WASM build.
        ///
        /// # Errors
        ///
        /// Returns [`Error::MissingFile`] if either ONNX file is absent, or
        /// [`Error::Model`] if the ONNX Runtime session cannot be built.
        pub fn from_dir<P: AsRef<Path>>(model_dir: P) -> Result<Self> {
            let dir = model_dir.as_ref();
            let encoder_path = dir.join("encoder.onnx");
            let decoder_path = dir.join("decoder_joint_fp32.onnx");

            if !encoder_path.exists() {
                return Err(Error::MissingFile(encoder_path));
            }
            if !decoder_path.exists() {
                return Err(Error::MissingFile(decoder_path));
            }

            let encoder = Session::builder()?.commit_from_file(&encoder_path)?;
            let decoder_joint = Session::builder()?.commit_from_file(&decoder_path)?;

            Ok(Self {
                encoder,
                decoder_joint,
            })
        }
    }

    /// Extract a tensor output by name into an owned `Vec` plus its `usize`
    /// dimensions.
    ///
    /// ONNX tensor shapes are `i64`; we cast to `usize` here. The values are
    /// always non-negative model architecture constants (`HIDDEN_DIM`,
    /// `LSTM_LAYERS`, etc.) -- a shape dim of >= 2^63 is physically impossible.
    fn extract_f32(
        outputs: &ort::session::SessionOutputs,
        name: &str,
    ) -> Result<(Vec<usize>, Vec<f32>)> {
        let (shape, data) = outputs[name]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::Model(format!("failed to extract `{name}`: {e}")))?;
        // Tensor shape dims are always non-negative model constants (see doc).
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let dims = shape.as_ref().iter().map(|&d| d as usize).collect();
        Ok((dims, data.to_vec()))
    }

    fn extract_i64(
        outputs: &ort::session::SessionOutputs,
        name: &str,
    ) -> Result<(Vec<usize>, Vec<i64>)> {
        let (shape, data) = outputs[name]
            .try_extract_tensor::<i64>()
            .map_err(|e| Error::Model(format!("failed to extract `{name}`: {e}")))?;
        // Same rationale as extract_f32.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let dims = shape.as_ref().iter().map(|&d| d as usize).collect();
        Ok((dims, data.to_vec()))
    }

    impl AsrBackend for OrtBackend {
        fn run_encoder(
            &mut self,
            features: &Array3<f32>,
            length: i64,
            cache: &EncoderCache,
        ) -> Result<EncoderOutput> {
            let length_arr = Array1::from_vec(vec![length]);

            let outputs = self.encoder.run(ort::inputs![
                "processed_signal" => Value::from_array(features.clone())?,
                "processed_signal_length" => Value::from_array(length_arr)?,
                "cache_last_channel" => Value::from_array(cache.last_channel.clone())?,
                "cache_last_time" => Value::from_array(cache.last_time.clone())?,
                "cache_last_channel_len" => Value::from_array(cache.last_channel_len.clone())?
            ])?;

            let (enc_dims, enc_data) = extract_f32(&outputs, "encoded")?;
            let encoded =
                Array3::from_shape_vec((enc_dims[0], enc_dims[1], enc_dims[2]), enc_data)?;

            let (_, len_data) = extract_i64(&outputs, "encoded_len")?;
            let encoded_len = len_data[0];

            let (ch_dims, ch_data) = extract_f32(&outputs, "cache_last_channel_next")?;
            let last_channel =
                Array4::from_shape_vec((ch_dims[0], ch_dims[1], ch_dims[2], ch_dims[3]), ch_data)?;

            let (tm_dims, tm_data) = extract_f32(&outputs, "cache_last_time_next")?;
            let last_time =
                Array4::from_shape_vec((tm_dims[0], tm_dims[1], tm_dims[2], tm_dims[3]), tm_data)?;

            let (chan_len_dims, chan_len_data) =
                extract_i64(&outputs, "cache_last_channel_len_next")?;
            let last_channel_len = Array1::from_shape_vec(chan_len_dims[0], chan_len_data)?;

            Ok(EncoderOutput {
                encoded,
                encoded_len,
                cache: EncoderCache {
                    last_channel,
                    last_time,
                    last_channel_len,
                },
            })
        }

        fn run_decoder(
            &mut self,
            encoder_frame: &Array3<f32>,
            target_token: i32,
            state_1: &Array3<f32>,
            state_2: &Array3<f32>,
        ) -> Result<DecoderOutput> {
            let targets = Array2::from_shape_vec((1, 1), vec![target_token])?;
            let target_len = Array1::from_vec(vec![1i32]);

            let outputs = self.decoder_joint.run(ort::inputs![
                "encoder_outputs" => Value::from_array(encoder_frame.clone())?,
                "targets" => Value::from_array(targets)?,
                "target_length" => Value::from_array(target_len)?,
                "input_states_1" => Value::from_array(state_1.clone())?,
                "input_states_2" => Value::from_array(state_2.clone())?
            ])?;

            let (_, logits_data) = extract_f32(&outputs, "outputs")?;
            let logits = Array1::from_vec(logits_data);

            let (h_dims, h_data) = extract_f32(&outputs, "output_states_1")?;
            let state_1 = Array3::from_shape_vec((h_dims[0], h_dims[1], h_dims[2]), h_data)?;

            let (c_dims, c_data) = extract_f32(&outputs, "output_states_2")?;
            let state_2 = Array3::from_shape_vec((c_dims[0], c_dims[1], c_dims[2]), c_data)?;

            Ok(DecoderOutput {
                logits,
                state_1,
                state_2,
            })
        }
    }
}

/// Slice a single encoder frame `t` out of `[1, HIDDEN_DIM, T]` into the
/// `[1, HIDDEN_DIM, 1]` shape the decoder expects.
///
/// Pure `ndarray` (no backend dependency), so it is available on both native
/// and wasm targets.
///
/// # Errors
///
/// Returns [`crate::error::Error::Model`] if the shape conversion fails
/// (indicates a bug in the encoder output shape).
pub(crate) fn encoder_frame(encoded: &Array3<f32>, t: usize) -> Result<Array3<f32>> {
    use ndarray::Axis;
    let hidden = encoded.shape()[1];
    // Remove the batch dim (axis 0, index 0), then remove the time dim
    // (axis 1, index t) -- avoids the `s![]` macro whose expansion contains
    // an ndarray-internal `#[allow(unsafe_code)]` that conflicts with
    // `workspace.lints.rust.unsafe_code = "forbid"` (E0453).
    let frame = encoded
        .index_axis(Axis(0), 0)
        .index_axis(Axis(1), t)
        .to_owned();
    Ok(frame.to_shape((1, hidden, 1))?.to_owned())
}
