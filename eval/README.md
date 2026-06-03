# Speaker-embedder bake-off

This is the evaluation harness behind the diarization claim in the app
(`index.html`, `class SpeakerTracker`): that the app's original 20 hand-engineered
acoustic features were near-random for speaker ID, and that **NeMo TitaNet-small**
is the right learned replacement to run in-browser.

It is committed so the claim is checkable rather than asserted. Numbers below are
reproduced verbatim from [`bench_results.json`](bench_results.json).

## What it measures

`bench.py` runs every candidate through **sherpa-onnx's own
`SpeakerEmbeddingExtractor`** (so each model gets exactly the feature front-end it
was trained on — the same path the browser WASM build uses), on one labeled
multi-speaker set, and reports:

- **within / across** speaker mean cosine, and the **gap** between them (bigger = better separation)
- **EER** — threshold-free verification equal-error-rate (lower = better)
- **greedy "leader" clustering** that mirrors the app's live `identify()`, run on a
  meeting-like *interleaved* turn order at the EER-optimal threshold: **clusters
  found vs. true**, and **purity**
- mean embed **latency** per utterance (native CPU; WASM is ~2–4× slower but proportional)

The baseline (`BASELINE_20feat`) is a faithful NumPy port of the app's old
`computeFingerprint()` (the 20 hand features), scored on the identical audio.

## The test set — read this before quoting the numbers

**6 speakers from LibriSpeech `test-clean`, 10 utterances each = 60 utterances**
(`audio/<speaker_id>/<speaker>-<chapter>-<utt>.flac`; speakers 121, 1089, 1188,
1221, 1284, 1320).

LibriSpeech is **clean, read audiobook speech**. So "EER 0%" is real, but it is on
*easy* audio — clean, single-speaker-per-clip, no overlap, no room/mic noise. It
says the embeddings **separate speakers extremely well when the audio is clean**;
it does **not** claim 0% on live meeting audio. Real meetings (short, noisy,
overlapping segments) compress that separation — which is exactly why the *live
clustering*, not the embeddings, is the app's weak link (see
[`../docs/DIARIZATION.md`](../docs/DIARIZATION.md)). Treat this as an
embedding-model selection benchmark, not a diarization-accuracy claim.

## Results (LibriSpeech test-clean, 6×10)

| model | dim | MB | gap | EER | clusters (found/true) | purity |
|---|---:|---:|---:|---:|:--:|---:|
| **nemo_en_titanet_small** ✅ winner | 192 | 40 | **0.784** | **0.0%** | 6/6 | 1.00 |
| 3dspeaker_eres2net | 192 | 27 | 0.730 | 0.0% | 6/6 | 1.00 |
| nemo_speakernet | 256 | 23 | 0.605 | 0.7% | 6/6 | 1.00 |
| wespeaker_resnet34_LM | 256 | 27 | 0.353 | 4.5% | 6/6 | 1.00 |
| 3dspeaker_campplus | 512 | 30 | 0.229 | 35.2% | 10/6 | 0.48 |
| wespeaker_CAM++_LM | 512 | 29 | 0.091 | 41.1% | 5/6 | 0.38 |
| *BASELINE_20feat (old hand features)* | 20 | 0 | 0.146 | 36.7% | **17/6** | 0.57 |

The headline: the old 20-feature method was **near-random** (EER 36.7%, 6 speakers
shattered into 17 clusters). TitaNet-small wins on separation gap and ties for best
EER at a far smaller dim than the 512-d models, which is why it's the one shipped.

## Reproduce

Heavy binaries are **not** committed (models ≈ 175 MB, LibriSpeech audio, venvs).
You supply them:

```bash
cd eval
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt

# 1) audio/  — 6 speakers from LibriSpeech test-clean, one dir each:
#    audio/1089/1089-134686-0000.flac ...   (any 6 speakers, ~10 clips each)
#    https://www.openslr.org/12  (test-clean)

# 2) models/  — the 6 ONNX speaker embedders, from the sherpa-onnx model zoo:
#    https://github.com/k2-fsa/sherpa-onnx/releases  (tag: speaker-recognition-models)
#    files: nemo_en_titanet_small.onnx, 3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx,
#           nemo_en_speakerverification_speakernet.onnx, wespeaker_en_voxceleb_resnet34_LM.onnx,
#           3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx, wespeaker_en_voxceleb_CAM++_LM.onnx

python bench.py        # prints the table, rewrites bench_results.json
```

## The pure-JS front-end, byte-validated (the `cosine 1.000000` claim)

The app reimplements TitaNet's NeMo mel front-end in pure JS so it can run under
onnxruntime-web with no Python. These scripts prove that port is correct:

- `featurize_match.py` — sweeps the uncertain NeMo featurizer knobs (htk vs slaney
  mel, periodic window, preemphasis, log-guard) and scores each by cosine to the
  sherpa-onnx ground truth. Pins the recipe: slaney 80-mel, periodic Hann, preemph 0.97.
- `dump_for_js.py` — writes the exact 80×257 slaney mel matrix (`mel_fb.json`, the
  same one shipped at the repo root and embedded in the app) plus per-clip reference
  `{samples, feat, emb}` dumps.
- `js/melfeat.mjs` — the pure-JS log-mel front-end (same code path as the browser).
- `js/validate.mjs` — runs `melfeat.mjs` against the Python reference dumps and
  asserts **feature max-abs-diff < 1e-3 and embedding cosine > 0.999**. On the
  reference clips it reports **cosine 1.000000** — i.e. the JS port reproduces the
  Python implementation it mirrors. (Separately, that Python front-end approximates
  sherpa-onnx's internal featurizer at ~0.94 cosine — close, not identical; the
  shipped path uses this same recipe end to end.)

```bash
cd eval/js
npm install                       # onnxruntime-node
# needs: mel_fb.json (committed), titanet.onnx, and ref/ dumps from dump_for_js.py
node validate.mjs
```
