#!/usr/bin/env python
"""
Speaker-embedder bake-off for the Silent Notetaker.

Compares learned speaker-embedding ONNX models (run through sherpa-onnx's own
SpeakerEmbeddingExtractor, so each model gets exactly the feature front-end it
was trained on — same as the browser WASM build) against the app's current
20 hand-engineered acoustic features (SpeakerTracker.computeFingerprint).

Metrics, all on the SAME labeled multi-speaker audio:
  - within/across-speaker mean cosine, and the gap (bigger = better separation)
  - EER  (threshold-free verification error; lower = better)
  - greedy "leader" diarization (mirrors app's identify()) on a meeting-like
    interleaved turn order, at the EER-optimal threshold:
        clusters found vs true, and purity
  - mean embed latency per utterance (native; WASM ~2-4x slower but proportional)
"""
import os, glob, time, json
import numpy as np
import soundfile as sf

AUDIO_DIR = "audio"
MODEL_DIR = "models"
SR = 16000


# ───────────────────────── audio ─────────────────────────
def load_wav(path):
    x, sr = sf.read(path, dtype="float32")
    if x.ndim > 1:
        x = x.mean(axis=1)
    if sr != SR:
        import librosa
        x = librosa.resample(x, orig_sr=sr, target_sr=SR)
        sr = SR
    return np.ascontiguousarray(x.astype(np.float32)), sr


def load_dataset():
    """Returns {speaker_id: [(utt_id, samples), ...]}"""
    data = {}
    for spk in sorted(os.listdir(AUDIO_DIR)):
        d = os.path.join(AUDIO_DIR, spk)
        if not os.path.isdir(d):
            continue
        utts = []
        files = sorted(glob.glob(os.path.join(d, "*.wav")) + glob.glob(os.path.join(d, "*.flac")))
        for w in files:
            x, _ = load_wav(w)
            utts.append((os.path.basename(w), x))
        if utts:
            data[spk] = utts
    return data


# ───────────────── baseline: 20 hand features ─────────────────
def compute_fingerprint(samples):
    """Faithful port of SpeakerTracker.computeFingerprint (index.html:1886)."""
    s = samples.astype(np.float64)
    n = len(s)
    if n < 1600:
        return None
    f = np.zeros(20, dtype=np.float64)
    f[0] = np.sqrt(np.sum(s * s) / n)                                  # RMS
    sign = s >= 0
    f[1] = np.sum(sign[1:] != sign[:-1]) / n                           # ZCR
    for b, lag in enumerate([20, 40, 80, 160]):                        # autocorr bands
        f[2 + b] = np.sum(s[:n - lag] * s[lag:]) / (n - lag)
    maxc, best = 0.0, 0                                                # pitch
    for lag in range(30, 300):
        m = min(4000, n - lag)
        if m <= 0:
            break
        c = float(np.sum(s[:m] * s[lag:lag + m]))
        if c > maxc:
            maxc, best = c, lag
    f[6] = 16000 / best if best > 0 else 0
    fft = 1024                                                         # "spectral" band energies
    start = max(0, n // 2 - fft // 2)
    win = s[start:start + fft]
    bsz = fft // 12
    for b in range(6):
        seg = win[b * bsz:min((b + 1) * bsz, len(win))]
        f[7 + b] = np.sqrt(np.sum(seg * seg) / bsz)
    q = n // 4                                                         # temporal quarters
    for qi in range(4):
        seg = s[qi * q:min((qi + 1) * q, n)]
        f[13 + qi] = np.sqrt(np.sum(seg * seg) / q) if q > 0 else 0
    emean, evar, fr, peaks, prev, mn, mx = f[0], 0.0, 400, 0, 0.0, np.inf, 0.0
    for fs in range(0, n - fr, fr):
        fe = np.sqrt(np.sum(s[fs:fs + fr] ** 2) / fr)
        evar += (fe - emean) ** 2
        thr = emean if fs > fr else fe
        if fe > prev and prev < thr:
            peaks += 1
        prev = fe
        mn, mx = min(mn, fe), max(mx, fe)
    denom = n / fr
    f[17], f[18], f[19] = evar / denom, peaks / denom, mx - mn
    norm = np.sqrt(np.sum(f * f))
    if norm > 0:
        f = f / norm
    return f.astype(np.float32)


# ───────────────── learned embedder via sherpa-onnx ─────────────────
def make_extractor(model_path):
    import sherpa_onnx
    cfg = sherpa_onnx.SpeakerEmbeddingExtractorConfig(
        model=model_path, num_threads=1, debug=False, provider="cpu"
    )
    return sherpa_onnx.SpeakerEmbeddingExtractor(cfg)


def embed_sherpa(ext, samples):
    st = ext.create_stream()
    st.accept_waveform(SR, samples)
    st.input_finished()
    return np.array(ext.compute(st), dtype=np.float32)


# ───────────────────────── metrics ─────────────────────────
def l2(v):
    n = np.linalg.norm(v, axis=-1, keepdims=True)
    return v / np.maximum(n, 1e-9)


def metrics(embs_by_spk):
    spks = list(embs_by_spk.keys())
    vecs, labs = [], []
    for s in spks:
        for v in embs_by_spk[s]:
            vecs.append(v)
            labs.append(s)
    V = l2(np.array(vecs, dtype=np.float64))
    labs = np.array(labs)
    S = V @ V.T
    N = len(labs)
    pos, neg = [], []
    for i in range(N):
        for j in range(i + 1, N):
            (pos if labs[i] == labs[j] else neg).append(S[i, j])
    pos, neg = np.array(pos), np.array(neg)

    # EER
    ts = np.unique(np.concatenate([pos, neg]))
    eer, eer_t = 1.0, 0.5
    for t in ts:
        far = float(np.mean(neg >= t))
        frr = float(np.mean(pos < t))
        if abs(far - frr) < abs((eer if eer_t == t else 1.0)):
            pass
    # cleaner sweep
    best_gap, eer, eer_t = 9, 1.0, 0.5
    for t in ts:
        far = float(np.mean(neg >= t))
        frr = float(np.mean(pos < t))
        if abs(far - frr) < best_gap:
            best_gap, eer, eer_t = abs(far - frr), (far + frr) / 2, float(t)

    # meeting-like interleaved leader clustering at EER threshold
    order = []
    maxlen = max(len(embs_by_spk[s]) for s in spks)
    for k in range(maxlen):
        for s in spks:
            if k < len(embs_by_spk[s]):
                order.append((s, l2(embs_by_spk[s][k].astype(np.float64))))
    centroids, members = [], []   # centroids[i]=vec, members[i]=[true labels]
    assign = []
    for true, v in order:
        if centroids:
            sims = [float(v @ c) for c in centroids]
            bi = int(np.argmax(sims))
            if sims[bi] >= eer_t:
                members[bi].append(true)
                centroids[bi] = l2(centroids[bi] * len(members[bi]) + v)  # running mean (renorm)
                assign.append(bi)
                continue
        centroids.append(v.copy())
        members.append([true])
        assign.append(len(centroids) - 1)
    purity = sum(max(np.bincount([hash(x) % 9999 for x in m]).max() for _ in [0])
                 for m in members) / len(order)
    # robust purity
    tot = 0
    for m in members:
        vals, cnts = np.unique(m, return_counts=True)
        tot += cnts.max()
    purity = tot / len(order)

    return {
        "within": float(pos.mean()),
        "across": float(neg.mean()),
        "gap": float(pos.mean() - neg.mean()),
        "eer": float(eer),
        "eer_threshold": float(eer_t),
        "clusters_found": len(centroids),
        "clusters_true": len(spks),
        "purity": float(purity),
        "n_utts": len(order),
    }


# ───────────────────────── run ─────────────────────────
def main():
    data = load_dataset()
    nspk = len(data)
    nutt = sum(len(v) for v in data.values())
    print(f"Dataset: {nspk} speakers, {nutt} utterances\n")
    if nspk < 2:
        print("Need >=2 speakers. Abort.")
        return

    results = {}

    # baseline
    t0 = time.perf_counter()
    embs = {s: [compute_fingerprint(x) for _, x in utts] for s, utts in data.items()}
    embs = {s: [e for e in v if e is not None] for s, v in embs.items()}
    lat = (time.perf_counter() - t0) / nutt * 1000
    m = metrics(embs)
    m["latency_ms"] = lat
    m["dim"] = 20
    m["size_mb"] = 0.0
    results["BASELINE_20feat"] = m

    # learned models
    for mp in sorted(glob.glob(os.path.join(MODEL_DIR, "*.onnx"))):
        name = os.path.basename(mp)[:-5]
        try:
            ext = make_extractor(mp)
        except Exception as e:
            print(f"  ! {name}: load failed: {e}")
            continue
        embs, t0, cnt = {}, time.perf_counter(), 0
        for s, utts in data.items():
            embs[s] = []
            for _, x in utts:
                embs[s].append(embed_sherpa(ext, x))
                cnt += 1
        lat = (time.perf_counter() - t0) / cnt * 1000
        m = metrics(embs)
        m["latency_ms"] = lat
        m["dim"] = len(embs[list(embs)[0]][0])
        m["size_mb"] = round(os.path.getsize(mp) / 1e6, 1)
        results[name] = m
        print(f"  ✓ {name}: gap={m['gap']:.3f} EER={m['eer']*100:.1f}% "
              f"clusters={m['clusters_found']}/{m['clusters_true']} purity={m['purity']:.2f}")

    # table
    print("\n" + "=" * 110)
    hdr = f"{'model':<42}{'dim':>5}{'MB':>7}{'within':>8}{'across':>8}{'gap':>7}{'EER%':>7}{'clust':>7}{'purity':>8}{'ms':>8}"
    print(hdr)
    print("-" * 110)
    order = sorted(results.items(), key=lambda kv: kv[1]["eer"])
    for name, m in order:
        print(f"{name:<42}{m['dim']:>5}{m['size_mb']:>7}{m['within']:>8.3f}{m['across']:>8.3f}"
              f"{m['gap']:>7.3f}{m['eer']*100:>7.1f}{m['clusters_found']:>4}/{m['clusters_true']:<2}"
              f"{m['purity']:>8.2f}{m['latency_ms']:>8.2f}")
    print("=" * 110)
    json.dump(results, open("bench_results.json", "w"), indent=2)
    print("\nSaved bench_results.json")


if __name__ == "__main__":
    main()
