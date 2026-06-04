#!/usr/bin/env python3
"""Golden reference harness for nemotron-speech-streaming-en-0.6b (INT8 ONNX).

Mirrors altunenes/parakeet-rs `transcribe_audio` + `decode_chunk` EXACTLY, in
Python on onnxruntime. Purpose: prove the model + INT8 weights + streaming
protocol actually transcribe correctly. onnxruntime-Python uses the same op
kernels as onnxruntime-web, so a pass here de-risks the Rust/ort-web (B1) path.

Run:  ./venv/bin/python golden_harness.py test_16k.wav
"""
import sys, time, wave
import numpy as np
import onnxruntime as ort
import librosa
import sentencepiece as spm

# --- constants (verbatim from parakeet-rs nemotron.rs) ---
SR, N_FFT, WIN, HOP, N_MELS = 16000, 512, 400, 160, 128
PREEMPH = 0.97
LOG_GUARD = 2.0 ** -24                 # NeMo log_zero_guard, == 5.9604645e-8
CHUNK, PRE = 56, 9                      # 560ms chunk + pre-encode lookback => 65 mel frames
NUM_LAYERS, HIDDEN, LEFT_CTX, CONV_CTX = 24, 1024, 70, 8
VOCAB, BLANK, LSTM_DIM, LSTM_LAYERS = 1024, 1024, 640, 2
MAX_SYM = 10                           # max symbols emitted per encoder frame

MODELS = "models"

def read_wav_mono16k(path):
    w = wave.open(path, "rb")
    assert w.getframerate() == SR, f"expected {SR}Hz, got {w.getframerate()}"
    assert w.getsampwidth() == 2, "expected 16-bit PCM"
    n, ch = w.getnframes(), w.getnchannels()
    raw = np.frombuffer(w.readframes(n), dtype=np.int16).astype(np.float32) / 32768.0
    if ch > 1:
        raw = raw.reshape(-1, ch).mean(axis=1)
    return raw

def compute_mel(audio):
    """preemph -> power STFT (window at frame start, shift-invariant for |X|^2)
    -> Slaney mel -> ln(x + 2^-24). NO normalization (Nemotron path)."""
    pe = np.empty_like(audio)
    pe[0] = audio[0]
    pe[1:] = audio[1:] - PREEMPH * audio[:-1]
    pad = N_FFT // 2
    padded = np.concatenate([np.zeros(pad, np.float32), pe, np.zeros(pad, np.float32)])
    window = np.hanning(WIN).astype(np.float32)          # symmetric Hann (matches parakeet-rs)
    nframes = (len(padded) - N_FFT) // HOP + 1
    freq_bins = N_FFT // 2 + 1
    spec = np.zeros((freq_bins, nframes), np.float32)
    frame = np.zeros(N_FFT, np.float32)
    for i in range(nframes):
        s = i * HOP
        seg = padded[s:s + WIN]
        frame[:] = 0.0
        frame[:len(seg)] = seg * window[:len(seg)]
        X = np.fft.rfft(frame)
        spec[:, i] = (X.real ** 2 + X.imag ** 2)         # power == realfft norm_sqr
    melfb = librosa.filters.mel(sr=SR, n_fft=N_FFT, n_mels=N_MELS,
                                fmin=0.0, fmax=SR / 2, htk=False, norm="slaney")  # [128,257]
    mel = melfb @ spec                                   # [128, T]
    return np.log(mel + LOG_GUARD).astype(np.float32)

def main(wav_path, decoder="decoder_joint.onnx"):
    audio = read_wav_mono16k(wav_path)
    mel = compute_mel(audio)
    total = mel.shape[1]
    print(f"audio: {len(audio)/SR:.2f}s | mel: {mel.shape} | log-mel range [{mel.min():.2f},{mel.max():.2f}] | decoder={decoder}")

    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    enc = ort.InferenceSession(f"{MODELS}/encoder.onnx", so, providers=["CPUExecutionProvider"])
    dec = ort.InferenceSession(f"{MODELS}/{decoder}", so, providers=["CPUExecutionProvider"])
    sp = spm.SentencePieceProcessor(model_file=f"{MODELS}/tokenizer.model")

    # streaming state
    cache_ch = np.zeros((NUM_LAYERS, 1, LEFT_CTX, HIDDEN), np.float32)
    cache_t = np.zeros((NUM_LAYERS, 1, HIDDEN, CONV_CTX), np.float32)
    cache_len = np.zeros((1,), np.int64)
    state1 = np.zeros((LSTM_LAYERS, 1, LSTM_DIM), np.float32)
    state2 = np.zeros((LSTM_LAYERS, 1, LSTM_DIM), np.float32)
    last_token = BLANK

    all_tokens, partials = [], []
    n_enc_calls = n_dec_calls = 0
    buf = chunk_idx = 0
    t0 = time.time()
    while buf < total:
        chunk_end = min(buf + CHUNK, total)
        main_len = chunk_end - buf
        exp = PRE + CHUNK
        chunk = np.zeros((1, N_MELS, exp), np.float32)
        if chunk_idx > 0 and buf >= PRE:
            chunk[0, :, 0:PRE] = mel[:, buf - PRE:buf]
        chunk[0, :, PRE:PRE + main_len] = mel[:, buf:buf + main_len]
        chunk_length = PRE + main_len

        eo = enc.run(None, {
            "processed_signal": chunk,
            "processed_signal_length": np.array([chunk_length], np.int64),
            "cache_last_channel": cache_ch,
            "cache_last_time": cache_t,
            "cache_last_channel_len": cache_len,
        })
        n_enc_calls += 1
        eo = {o.name: v for o, v in zip(enc.get_outputs(), eo)}
        encoded = eo["encoded"]                 # [1, 1024, T_enc]
        enc_frames = int(eo["encoded_len"][0])
        cache_ch, cache_t, cache_len = eo["cache_last_channel_next"], eo["cache_last_time_next"], eo["cache_last_channel_len_next"]

        chunk_tokens = []
        for t in range(enc_frames):
            frame = encoded[0, :, t].reshape(1, HIDDEN, 1).astype(np.float32)
            for _ in range(MAX_SYM):
                do = dec.run(None, {
                    "encoder_outputs": frame,
                    "targets": np.array([[last_token]], np.int32),
                    "target_length": np.array([1], np.int32),
                    "input_states_1": state1,
                    "input_states_2": state2,
                })
                n_dec_calls += 1
                do = {o.name: v for o, v in zip(dec.get_outputs(), do)}
                logits = do["outputs"].reshape(-1)        # 1025
                idx = int(np.argmax(logits))
                if idx == BLANK:
                    break
                chunk_tokens.append(idx)
                last_token = idx
                state1, state2 = do["output_states_1"], do["output_states_2"]
        all_tokens.extend(chunk_tokens)
        if chunk_tokens:
            partials.append(sp.decode([t for t in chunk_tokens if t < VOCAB]))
        buf += CHUNK
        chunk_idx += 1

    elapsed = time.time() - t0
    valid = [t for t in all_tokens if t < VOCAB]
    text_lib = sp.decode(valid)
    # manual piece join cross-check (matches parakeet-rs SentencePieceVocab.decode)
    text_manual = "".join(sp.id_to_piece(t) for t in valid).replace("▁", " ").strip()

    print("\n=== streaming partials (per 560ms chunk) ===")
    for i, p in enumerate(partials):
        print(f"  chunk {i:2d}: {p!r}")
    print("\n=== FINAL TRANSCRIPT (sp.decode) ===")
    print(" ", repr(text_lib))
    print("=== FINAL TRANSCRIPT (manual piece join) ===")
    print(" ", repr(text_manual))
    audio_s = len(audio) / SR
    print(f"\nchunks={chunk_idx} enc_calls={n_enc_calls} dec_calls={n_dec_calls} tokens={len(valid)}")
    print(f"compute={elapsed:.2f}s for {audio_s:.2f}s audio  =>  RTF={elapsed/audio_s:.3f}x (CPU, 4 threads)")

if __name__ == "__main__":
    wav = sys.argv[1] if len(sys.argv) > 1 else "test_16k.wav"
    decoder = sys.argv[2] if len(sys.argv) > 2 else "decoder_joint.onnx"
    main(wav, decoder)
