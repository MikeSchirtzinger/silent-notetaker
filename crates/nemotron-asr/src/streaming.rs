//! Cache-aware streaming RNN-T decode loop.
//!
//! This ports `parakeet-rs`'s `transcribe_audio` + `decode_chunk` (the same
//! logic the Python golden harness mirrors). The encoder is run over fixed
//! 56-frame mel chunks — each prefixed with 9 lookback frames — and its cache
//! is carried forward. A greedy RNN-T loop then emits up to 10 tokens per
//! encoder frame, advancing the LSTM state and `last_token` only when a
//! non-blank token is produced.

use ndarray::Array3;

use crate::audio::MelFrontend;
use crate::constants::{
    BLANK_ID, CHUNK_SIZE, DECODER_LSTM_DIM, LSTM_LAYERS, MAX_SYMBOLS_PER_STEP, N_MELS,
    PRE_ENCODE_CACHE, VOCAB_SIZE,
};
use crate::error::{Error, Result};
use crate::model::{encoder_frame, AsrBackend, EncoderCache};
use crate::vocab::SentencePieceVocab;

/// Mutable per-utterance decoder state: encoder cache, LSTM state, and the
/// last emitted token.
struct DecodeState {
    encoder_cache: EncoderCache,
    state_1: Array3<f32>,
    state_2: Array3<f32>,
    last_token: i32,
}

impl DecodeState {
    fn fresh() -> Self {
        Self {
            encoder_cache: EncoderCache::new(),
            state_1: Array3::zeros((LSTM_LAYERS, 1, DECODER_LSTM_DIM)),
            state_2: Array3::zeros((LSTM_LAYERS, 1, DECODER_LSTM_DIM)),
            last_token: BLANK_ID as i32,
        }
    }
}

/// Streaming ASR engine generic over the inference [`AsrBackend`].
///
/// Holds the mel front-end and the tokenizer alongside the swappable backend.
/// Use [`StreamingAsr::transcribe_audio`] for offline transcription (the
/// validated path); [`StreamingAsr::decode_chunk`] is exposed for live mic
/// streaming where the caller drives chunking.
pub struct StreamingAsr<B: AsrBackend> {
    backend: B,
    frontend: MelFrontend,
    vocab: SentencePieceVocab,
    state: DecodeState,
}

impl<B: AsrBackend> StreamingAsr<B> {
    /// Construct from a loaded backend, mel front-end, and vocabulary.
    pub fn new(backend: B, frontend: MelFrontend, vocab: SentencePieceVocab) -> Self {
        Self {
            backend,
            frontend,
            vocab,
            state: DecodeState::fresh(),
        }
    }

    /// Reset all streaming state for a new utterance.
    pub fn reset(&mut self) {
        self.state = DecodeState::fresh();
    }

    /// Transcribe a full audio buffer offline.
    ///
    /// Computes the complete log-mel spectrogram, then slides a 56-frame window
    /// (`buffer_idx += CHUNK_SIZE`) over it. Each chunk is built as
    /// `[1, N_MELS, PRE_ENCODE_CACHE + CHUNK_SIZE]`: the first
    /// [`PRE_ENCODE_CACHE`] slots are lookback frames from the previous chunk
    /// (zero for the first chunk), followed by up to [`CHUNK_SIZE`] main frames.
    /// The encoder runs with `length = PRE_ENCODE_CACHE + main_len`, its
    /// `*_next` caches are carried forward, and [`Self::decode_chunk`] greedily
    /// decodes the returned encoder frames.
    pub fn transcribe_audio(&mut self, audio: &[f32]) -> Result<String> {
        self.reset();

        let mel = self.frontend.log_mel(audio)?;
        let total_frames = mel.shape()[1];
        if total_frames == 0 {
            return Ok(String::new());
        }

        let expected_size = PRE_ENCODE_CACHE + CHUNK_SIZE;
        let mut all_tokens: Vec<usize> = Vec::new();
        let mut buffer_idx = 0;
        let mut chunk_idx = 0;

        while buffer_idx < total_frames {
            let chunk_end = (buffer_idx + CHUNK_SIZE).min(total_frames);
            let main_len = chunk_end - buffer_idx;

            // Layout is [N_MELS, expected_size] flattened row-major
            // (mel-major), matching the encoder's [1, N_MELS, T] input.
            let mut chunk_data = vec![0.0f32; N_MELS * expected_size];

            // Pre-encode lookback: the 9 frames immediately preceding this
            // chunk (only available from the second chunk onward).
            if chunk_idx > 0 && buffer_idx >= PRE_ENCODE_CACHE {
                let cache_start = buffer_idx - PRE_ENCODE_CACHE;
                for f in 0..PRE_ENCODE_CACHE {
                    for m in 0..N_MELS {
                        chunk_data[m * expected_size + f] = mel[[m, cache_start + f]];
                    }
                }
            }

            // Main frames.
            for f in 0..main_len {
                for m in 0..N_MELS {
                    chunk_data[m * expected_size + PRE_ENCODE_CACHE + f] = mel[[m, buffer_idx + f]];
                }
            }

            let mel_chunk = Array3::from_shape_vec((1, N_MELS, expected_size), chunk_data)
                .map_err(|e| Error::Feature(format!("failed to build mel chunk: {e}")))?;
            let chunk_length = (PRE_ENCODE_CACHE + main_len) as i64;

            let enc =
                self.backend
                    .run_encoder(&mel_chunk, chunk_length, &self.state.encoder_cache)?;
            self.state.encoder_cache = enc.cache;

            let tokens = self.decode_chunk(&enc.encoded, enc.encoded_len as usize)?;
            all_tokens.extend(tokens);

            buffer_idx += CHUNK_SIZE;
            chunk_idx += 1;
        }

        let valid: Vec<usize> = all_tokens.into_iter().filter(|&t| t < VOCAB_SIZE).collect();
        Ok(self.vocab.decode(&valid))
    }

    /// Greedily decode the `enc_frames` encoder frames in `encoded`
    /// (`[1, HIDDEN_DIM, T]`), returning the token ids emitted.
    ///
    /// For each frame, up to [`MAX_SYMBOLS_PER_STEP`] decoder steps run. A
    /// blank (`argmax == BLANK_ID`) ends the frame; a non-blank token is
    /// emitted and the LSTM state plus `last_token` are advanced. State is
    /// **not** updated on blanks.
    pub fn decode_chunk(&mut self, encoded: &Array3<f32>, enc_frames: usize) -> Result<Vec<usize>> {
        let mut tokens = Vec::new();

        for t in 0..enc_frames {
            let frame = encoder_frame(encoded, t)?;

            for _ in 0..MAX_SYMBOLS_PER_STEP {
                let out = self.backend.run_decoder(
                    &frame,
                    self.state.last_token,
                    &self.state.state_1,
                    &self.state.state_2,
                )?;

                let max_idx = argmax(
                    out.logits
                        .as_slice()
                        .ok_or_else(|| Error::Model("decoder logits not contiguous".into()))?,
                );

                if max_idx == BLANK_ID {
                    break;
                }

                tokens.push(max_idx);
                self.state.last_token = max_idx as i32;
                self.state.state_1 = out.state_1;
                self.state.state_2 = out.state_2;
            }
        }

        Ok(tokens)
    }

    /// Detokenize a slice of token ids using the loaded vocabulary.
    pub fn decode_tokens(&self, ids: &[usize]) -> String {
        let valid: Vec<usize> = ids.iter().copied().filter(|&t| t < VOCAB_SIZE).collect();
        self.vocab.decode(&valid)
    }
}

/// Index of the maximum element (first on ties), matching `np.argmax`.
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

#[cfg(test)]
mod tests {
    use super::argmax;

    #[test]
    fn argmax_returns_first_max_on_ties() {
        assert_eq!(argmax(&[0.1, 0.5, 0.5, 0.2]), 1);
        assert_eq!(argmax(&[3.0, 1.0, 2.0]), 0);
    }
}
