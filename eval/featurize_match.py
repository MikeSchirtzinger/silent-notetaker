#!/usr/bin/env python
"""
Prove a hand-built NeMo mel front-end can drive nemo_en_titanet_small.onnx and
reproduce sherpa-onnx's embeddings — the prerequisite for an honest in-browser
onnxruntime-web port. Ground truth = sherpa-onnx SpeakerEmbeddingExtractor.

We sweep the uncertain NeMo featurizer knobs (htk vs slaney mel, periodic window,
preemphasis, log-guard) and score each config by mean cosine to sherpa across all
utterances. >0.99 = front-end matches; the JS port then just mirrors this recipe.
"""
import os, glob, itertools, numpy as np, soundfile as sf, librosa
import onnxruntime as ort
import sherpa_onnx

SR = 16000
MODEL = "models/nemo_en_titanet_small.onnx"
AUDIO_DIR = "audio"

# ── load audio ──
def load(p):
    x, sr = sf.read(p, dtype="float32")
    if x.ndim > 1: x = x.mean(1)
    if sr != SR: x = librosa.resample(x, orig_sr=sr, target_sr=SR)
    return np.ascontiguousarray(x.astype(np.float32))

data = {}
for spk in sorted(os.listdir(AUDIO_DIR)):
    d = os.path.join(AUDIO_DIR, spk)
    if os.path.isdir(d):
        data[spk] = [load(f) for f in sorted(glob.glob(d + "/*.flac") + glob.glob(d + "/*.wav"))]
utts = [(s, x) for s, xs in data.items() for x in xs]
labels = [s for s, _ in utts]
print(f"{len(data)} speakers, {len(utts)} utts")

# ── ground truth: sherpa ──
cfg = sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=MODEL, num_threads=1, provider="cpu")
ext = sherpa_onnx.SpeakerEmbeddingExtractor(cfg)
def sherpa_emb(x):
    st = ext.create_stream(); st.accept_waveform(SR, x); st.input_finished()
    return np.array(ext.compute(st), dtype=np.float64)
GT = np.array([sherpa_emb(x) for _, x in utts])
GT /= np.linalg.norm(GT, axis=1, keepdims=True)

# ── candidate NeMo mel front-end ──
sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])

def featurize(x, n_mels=80, n_fft=512, win=400, hop=160, htk=False, norm="slaney",
              periodic=True, preemph=0.97, log_guard=2**-24, fmax=8000.0):
    x = x.astype(np.float64)
    if preemph:
        x = np.concatenate([[x[0]], x[1:] - preemph * x[:-1]])
    window = librosa.filters.get_window("hann", win, fftbins=periodic)
    S = librosa.stft(x, n_fft=n_fft, hop_length=hop, win_length=win, window=window,
                     center=True, pad_mode="reflect")
    power = (np.abs(S) ** 2)
    mel_fb = librosa.filters.mel(sr=SR, n_fft=n_fft, n_mels=n_mels, fmin=0.0,
                                 fmax=fmax, htk=htk, norm=norm)
    mel = mel_fb @ power                       # [n_mels, T]
    logmel = np.log(mel + log_guard)
    m = logmel.mean(axis=1, keepdims=True)
    s = logmel.std(axis=1, keepdims=True)
    return ((logmel - m) / (s + 1e-5)).astype(np.float32)

def titanet_emb(feat):
    out = sess.run(["embs"], {"audio_signal": feat[None],
                              "length": np.array([feat.shape[1]], np.int64)})[0][0]
    return out.astype(np.float64)

def eval_cfg(**kw):
    E = []
    for _, x in utts:
        E.append(titanet_emb(featurize(x, **kw)))
    E = np.array(E); E /= np.linalg.norm(E, axis=1, keepdims=True)
    coss = np.sum(E * GT, axis=1)          # per-utt cosine to sherpa
    # bake-off separation metric on E
    S = E @ E.T; pos = []; neg = []
    for i in range(len(E)):
        for j in range(i + 1, len(E)):
            (pos if labels[i] == labels[j] else neg).append(S[i, j])
    pos, neg = np.array(pos), np.array(neg)
    ts = np.unique(np.concatenate([pos, neg])); eer = 1.0
    for t in ts:
        far, frr = np.mean(neg >= t), np.mean(pos < t)
        if abs(far - frr) < eer: eer = max(far, frr) if False else (far + frr) / 2 if abs(far-frr)<0.001 else eer
    # simpler EER
    bg, eer = 9, 1.0
    for t in ts:
        far, frr = float(np.mean(neg >= t)), float(np.mean(pos < t))
        if abs(far - frr) < bg: bg, eer = abs(far - frr), (far + frr) / 2
    return coss.mean(), coss.min(), pos.mean() - neg.mean(), eer

print("\nSweeping front-end configs (cosine-to-sherpa is the match metric):")
print(f"{'htk':>4}{'norm':>8}{'periodic':>9}{'preemph':>8}  {'meanCos':>8}{'minCos':>8}{'gap':>7}{'EER%':>7}")
best = None
for htk, norm, periodic, preemph in itertools.product(
        [False, True], ["slaney", None], [True, False], [0.97, 0.0]):
    mc, mn, gap, eer = eval_cfg(htk=htk, norm=norm, periodic=periodic, preemph=preemph)
    print(f"{str(htk):>4}{str(norm):>8}{str(periodic):>9}{preemph:>8.2f}  "
          f"{mc:>8.4f}{mn:>8.4f}{gap:>7.3f}{eer*100:>7.1f}")
    if best is None or mc > best[0]:
        best = (mc, dict(htk=htk, norm=norm, periodic=periodic, preemph=preemph), gap, eer)
print(f"\nBEST mean-cosine={best[0]:.4f}  cfg={best[1]}  gap={best[2]:.3f}  EER={best[3]*100:.1f}%")
