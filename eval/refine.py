#!/usr/bin/env python
"""Refine the matched front-end: sweep mag_power, std ddof, log-guard, fmax to
maximize cosine-to-sherpa beyond the 0.94 baseline. Base cfg already fixed:
htk=False, slaney, periodic hann, preemph=0.97."""
import os, glob, itertools, numpy as np, soundfile as sf, librosa
import onnxruntime as ort, sherpa_onnx

SR, MODEL, AUDIO_DIR = 16000, "models/nemo_en_titanet_small.onnx", "audio"

def load(p):
    x, sr = sf.read(p, dtype="float32")
    if x.ndim > 1: x = x.mean(1)
    if sr != SR: x = librosa.resample(x, orig_sr=sr, target_sr=SR)
    return np.ascontiguousarray(x.astype(np.float32))

utts = []
for spk in sorted(os.listdir(AUDIO_DIR)):
    d = os.path.join(AUDIO_DIR, spk)
    if os.path.isdir(d):
        for f in sorted(glob.glob(d + "/*.flac") + glob.glob(d + "/*.wav")):
            utts.append(load(f))

cfg = sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=MODEL, num_threads=1, provider="cpu")
ext = sherpa_onnx.SpeakerEmbeddingExtractor(cfg)
def sg(x):
    st = ext.create_stream(); st.accept_waveform(SR, x); st.input_finished()
    return np.array(ext.compute(st), np.float64)
GT = np.array([sg(x) for x in utts]); GT /= np.linalg.norm(GT, 1, keepdims=True) if False else np.linalg.norm(GT, axis=1, keepdims=True)

sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
MEL = {}
def feat(x, mag_power, ddof, log_guard, fmax, n_fft):
    x = x.astype(np.float64); x = np.concatenate([[x[0]], x[1:] - 0.97 * x[:-1]])
    w = librosa.filters.get_window("hann", 400, fftbins=True)
    S = np.abs(librosa.stft(x, n_fft=n_fft, hop_length=160, win_length=400, window=w, center=True, pad_mode="reflect"))
    power = S ** mag_power
    key = (n_fft, fmax)
    if key not in MEL:
        MEL[key] = librosa.filters.mel(sr=SR, n_fft=n_fft, n_mels=80, fmin=0.0, fmax=fmax, htk=False, norm="slaney")
    mel = MEL[key] @ power
    lm = np.log(mel + log_guard)
    m = lm.mean(1, keepdims=True); s = lm.std(1, ddof=ddof, keepdims=True)
    return ((lm - m) / (s + 1e-5)).astype(np.float32)

def emb(f):
    return sess.run(["embs"], {"audio_signal": f[None], "length": np.array([f.shape[1]], np.int64)})[0][0].astype(np.float64)

print(f"{'magP':>5}{'ddof':>5}{'logguard':>10}{'fmax':>7}{'nfft':>6}  {'meanCos':>8}{'minCos':>8}")
best = None
for mag_power, ddof, lg, fmax, n_fft in itertools.product(
        [2.0, 1.0], [0, 1], [2**-24, 1e-6, 1e-10], [8000.0], [512]):
    E = np.array([emb(feat(x, mag_power, ddof, lg, fmax, n_fft)) for x in utts])
    E /= np.linalg.norm(E, axis=1, keepdims=True)
    c = np.sum(E * GT, axis=1)
    print(f"{mag_power:>5}{ddof:>5}{lg:>10.0e}{fmax:>7.0f}{n_fft:>6}  {c.mean():>8.4f}{c.min():>8.4f}")
    if best is None or c.mean() > best[0]:
        best = (c.mean(), c.min(), dict(mag_power=mag_power, ddof=ddof, log_guard=lg))
print(f"\nBEST meanCos={best[0]:.4f} minCos={best[1]:.4f} cfg={best[2]}")
