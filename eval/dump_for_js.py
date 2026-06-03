#!/usr/bin/env python
"""Dump ground-truth artifacts for validating the JS front-end port:
 - mel_fb.json: the exact 80x257 slaney mel matrix (JS embeds this verbatim)
 - ref/<id>.json: {samples, feat[80][T], emb[192]} for a few clips so the JS
   implementation can prove feature- and embedding-level parity."""
import os, glob, json, numpy as np, soundfile as sf, librosa
import onnxruntime as ort

SR, MODEL = 16000, "models/nemo_en_titanet_small.onnx"
os.makedirs("artifacts/ref", exist_ok=True)

mel_fb = librosa.filters.mel(sr=SR, n_fft=512, n_mels=80, fmin=0.0, fmax=8000.0, htk=False, norm="slaney")
json.dump({"n_mels": 80, "n_freq": mel_fb.shape[1], "matrix": mel_fb.astype(float).tolist()},
          open("artifacts/mel_fb.json", "w"))
print("mel_fb:", mel_fb.shape)

def feat(x):
    x = x.astype(np.float64); x = np.concatenate([[x[0]], x[1:] - 0.97 * x[:-1]])
    w = librosa.filters.get_window("hann", 400, fftbins=True)
    S = np.abs(librosa.stft(x, n_fft=512, hop_length=160, win_length=400, window=w, center=True, pad_mode="reflect"))
    mel = mel_fb @ (S ** 2)
    lm = np.log(mel + 2**-24)
    return ((lm - lm.mean(1, keepdims=True)) / (lm.std(1, keepdims=True) + 1e-5)).astype(np.float32)

sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
def emb(f):
    return sess.run(["embs"], {"audio_signal": f[None], "length": np.array([f.shape[1]], np.int64)})[0][0]

# 3 clips from different speakers, trimmed to ~4s to keep JSON small
clips = []
for spk in sorted(os.listdir("audio"))[:3]:
    f = sorted(glob.glob(f"audio/{spk}/*.flac") + glob.glob(f"audio/{spk}/*.wav"))[0]
    clips.append((spk, f))
for spk, path in clips:
    x, sr = sf.read(path, dtype="float32")
    if x.ndim > 1: x = x.mean(1)
    x = x[:SR * 4].astype(np.float32)
    f = feat(x); e = emb(f)
    cid = f"{spk}_{os.path.basename(path).split('.')[0]}"
    json.dump({"sr": SR, "samples": x.tolist(), "feat": f.astype(float).tolist(),
               "emb": e.astype(float).tolist(), "T": int(f.shape[1])},
              open(f"artifacts/ref/{cid}.json", "w"))
    print("ref:", cid, "samples", len(x), "feat", f.shape, "|emb|", round(float(np.linalg.norm(e)), 3))
print("DUMP_DONE")
