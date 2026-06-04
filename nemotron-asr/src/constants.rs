//! Model and front-end constants for `nemotron-speech-streaming-en-0.6b`.
//!
//! These are fixed by the exported ONNX graphs and the NeMo preprocessor; do
//! not change them without re-validating against the golden transcript.

// --- Audio / mel front-end ---

/// Input sample rate (Hz).
pub const SAMPLE_RATE: usize = 16_000;
/// FFT size.
pub const N_FFT: usize = 512;
/// Analysis window length (samples). Shorter than `N_FFT`; the rest is zero.
pub const WIN_LENGTH: usize = 400;
/// Hop between frames (samples) — 10 ms at 16 kHz.
pub const HOP_LENGTH: usize = 160;
/// Number of mel filterbank channels.
pub const N_MELS: usize = 128;
/// Pre-emphasis coefficient.
pub const PREEMPH: f32 = 0.97;
/// NeMo additive `log_zero_guard` = 2^-24.
pub const LOG_ZERO_GUARD: f32 = 5.960_464_5e-8;

// --- Encoder architecture ---

/// FastConformer encoder layers (cache depth).
pub const NUM_ENCODER_LAYERS: usize = 24;
/// Encoder hidden dimension.
pub const HIDDEN_DIM: usize = 1024;
/// `cache_last_channel` left-context frames.
pub const LEFT_CONTEXT: usize = 70;
/// `cache_last_time` convolution context.
pub const CONV_CONTEXT: usize = 8;

// --- Decoder (prediction network + joint) ---

/// Number of real (non-blank) vocabulary tokens.
pub const VOCAB_SIZE: usize = 1024;
/// RNN-T blank token id (== `VOCAB_SIZE`).
pub const BLANK_ID: usize = 1024;
/// Prediction-network LSTM hidden dimension.
pub const DECODER_LSTM_DIM: usize = 640;
/// Prediction-network LSTM layer count.
pub const LSTM_LAYERS: usize = 2;

// --- Streaming chunk configuration ---

/// Main mel frames consumed per chunk (560 ms).
pub const CHUNK_SIZE: usize = 56;
/// Pre-encode lookback frames prepended to each chunk.
pub const PRE_ENCODE_CACHE: usize = 9;
/// Greedy RNN-T cap on tokens emitted per encoder frame.
pub const MAX_SYMBOLS_PER_STEP: usize = 10;
/// Trailing mel frames that must NOT be consumed from an in-flight (still
/// growing) live-stream buffer.
///
/// `stft` zero-pads `N_FFT/2 = 256` samples on the right, so when the mel is
/// computed over a buffer whose audio hasn't fully arrived yet, the final
/// frame's analysis window (`[f*HOP, f*HOP + WIN_LENGTH)` in padded coords,
/// i.e. up to `f*HOP + 144` past the last real sample) can cover synthetic
/// zeros instead of the audio that arrives next. Consuming that frame is the
/// root cause of the word-merge artifacts seen at 500 ms feeds ("intelligence
/// is" → "intelligences") and the occasional boundary glitch at 1 s feeds
/// (the leftover cycle hits the dirty edge frame every ~14 chunks). By the
/// derivation only the single final frame can be dirty (`(len mod HOP) < 144`),
/// so 2 would suffice; 3 buys an off-by-one margin for 30 ms of extra latency.
/// `finalize`/`flush_tail` intentionally ignore this guard — at end of stream
/// the right-edge padding is legitimate.
pub const EDGE_GUARD_FRAMES: usize = 3;
