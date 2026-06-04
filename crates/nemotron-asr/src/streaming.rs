//! Cache-aware streaming RNN-T decode loop.
//!
//! This ports `parakeet-rs`'s `transcribe_audio` + `decode_chunk` (the same
//! logic the Python golden harness mirrors). The encoder is run over fixed
//! 56-frame mel chunks -- each prefixed with 9 lookback frames -- and its
//! cache is carried forward. A greedy RNN-T loop then emits up to 10 tokens
//! per encoder frame, advancing the LSTM state and `last_token` only when a
//! non-blank token is produced.
//!
//! Mel-chunk construction is delegated to
//! [`crate::chunk_core::build_offline_mel_chunk`] (shared with the wasm
//! backend) and `argmax` to [`crate::chunk_core::argmax`] (shared with the
//! wasm backend).

use ndarray::Array3;

use crate::audio::MelFrontend;
use crate::chunk_core::{argmax, build_offline_mel_chunk};
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
        // BLANK_ID = 1024, which is well within i32 range (max 2^31 - 1).
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
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
    /// (`buffer_idx += CHUNK_SIZE`) over it. Each chunk is built by
    /// [`build_offline_mel_chunk`]: the first [`PRE_ENCODE_CACHE`] slots are
    /// lookback frames from the previous chunk (zero for the first chunk),
    /// followed by up to [`CHUNK_SIZE`] main frames. The encoder runs with
    /// `length = PRE_ENCODE_CACHE + main_len`, its `*_next` caches are carried
    /// forward, and [`Self::decode_chunk`] greedily decodes the returned
    /// encoder frames.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error`] if the mel frontend fails, if the
    /// encoder or decoder session fails, or if a tensor shape is inconsistent.
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

            let chunk_data = build_offline_mel_chunk(&mel, chunk_idx, buffer_idx, main_len);

            let mel_chunk = Array3::from_shape_vec((1, N_MELS, expected_size), chunk_data)
                .map_err(|e| Error::Feature(format!("failed to build mel chunk: {e}")))?;
            // PRE_ENCODE_CACHE + main_len <= 65, safely fits in i64.
            #[allow(clippy::cast_possible_wrap)]
            let chunk_length = (PRE_ENCODE_CACHE + main_len) as i64;

            let enc =
                self.backend
                    .run_encoder(&mel_chunk, chunk_length, &self.state.encoder_cache)?;
            self.state.encoder_cache = enc.cache;

            // encoded_len is a non-negative encoder output; <= CHUNK_SIZE = 56.
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let enc_frames = enc.encoded_len as usize;
            let tokens = self.decode_chunk(&enc.encoded, enc_frames)?;
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
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error`] if the decoder session fails or a
    /// tensor extraction fails.
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
                // max_idx <= VOCAB_SIZE = 1024, which fits safely in i32.
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                {
                    self.state.last_token = max_idx as i32;
                }
                self.state.state_1 = out.state_1;
                self.state.state_2 = out.state_2;
            }
        }

        Ok(tokens)
    }

    /// Detokenize a slice of token ids using the loaded vocabulary.
    #[must_use]
    pub fn decode_tokens(&self, ids: &[usize]) -> String {
        let valid: Vec<usize> = ids.iter().copied().filter(|&t| t < VOCAB_SIZE).collect();
        self.vocab.decode(&valid)
    }
}
