# Spike S2 — JS-host adapter round-trip (HOT-PATH GATE)

**Task:** B2 (`b2-spike-jshost`), board task #7, PRD Phase 0 S2.
**Date:** 2026-06-04 · **Branch context:** `rust-refactor`.
**Spike code:** `/Users/mike/dev/snt-spikes/b2-jshost/`.

## Verdict

**GATE: PASS.** Routing the hot audio path through a Rust/WASM policy core that
drives a transformers.js worker via a typed command protocol adds **no
measurable latency regression** versus the current direct JS loop. On the
rigorous back-to-back measurement, the per-chunk round-trip and boundary
overhead are **within inference noise** (≈6 ms run-to-run on a ~470 ms
inference), and the WASM policy call itself costs **0.14 ms p50**.

Phase I's Voxtral two-cap-recycle design can proceed on the `JsHostEngine`
boundary: the round-trip is not the bottleneck.

## What was built (no mocks — real model, real WASM, real boundary)

Both paths run the **same real model** (`onnx-community/whisper-tiny.en`, q4,
WASM EP via transformers.js v3) on the **same real audio**
(`nemotron-asr/test-assets/test_16k.wav`, 6.03 s of real 16 kHz mono speech,
looped to reach the chunk count) in the **same browser** (Chrome 148,
`crossOriginIsolated === true`, SharedArrayBuffer available), through the **same
host worker** (`transformers-host.js`). The only thing that varies is *who
decides to feed*:

| File | Role |
|---|---|
| `policy-core/` | **Real Rust/WASM crate** (`wasm-pack --target web`). `ChunkingPolicy::push_samples()` owns the accumulate + chunk + RMS-VAD policy and emits a typed `HostCommand` (`buffer` \| `feed{seq,len,dropped_silence}`) — the PRD's `JsHostEngine` command shape. Native unit tests (4/4) cover the policy without a browser. |
| `transformers-host.js` | The JS **host** — a transformers.js ASR executor with **no policy**. Receives `feed{seq,audio}`, runs the model, returns `result{seq,text,inferMs}`. Used unchanged by both paths. |
| `baseline.html` | **Current direct JS loop** — replicates `index.html`'s `startMoonshine`: plain-JS `audioBuffer.push` + `splice(0, CHUNK_SAMPLES)`, then `postMessage(..., [transfer])` straight to the worker. |
| `rust-host.html` | **Rust-on-the-hot-path** — every AudioWorklet-shaped quantum goes through `policy.push_samples(q)` (the WASM boundary); on a `feed` command, the returned chunk is posted to the *same* worker. |
| `harness.js` | Shared rig: WAV→PCM, AudioWorklet-shaped 128-sample render quanta, real-time pacing, p50/p95. |
| `config.js` | Single source of truth (model, chunk size, target chunks) so both paths are identical. |

### Why this is a real measurement, not a microbenchmark

- **Real model + real inference** on both paths — a fake "model" would make the
  delta meaningless (pre-completion checklist).
- **n = 120 chunks** per mode per path (≥100 required) for stable percentiles.
- **AudioWorklet-faithful input**: samples arrive as 128-sample quanta, exactly
  as `index.html`'s `CaptureProcessor.process()` posts them.
- **Two modes** to separate two questions:
  - **back-to-back** (one chunk fully round-trips before the next is fed) —
    isolates per-chunk round-trip cleanly; this is the gate number.
  - **realtime** (8 ms-paced quanta, fire-and-forget) — the production scenario;
    a sanity check that jitter sources are identical on both paths.
- The worker self-times inference (`inferMs`) so **boundary = roundTrip −
  inference** is computed per chunk, isolating postMessage + scheduling + the
  WASM call from the model cost.

## Measurements

### Back-to-back (serialized — the gate number), n = 120 each

| Metric | Baseline (direct JS) | Rust-host (WASM policy) | Delta |
|---|---|---|---|
| roundTrip p50 | 477.86 ms | 471.56 ms | **−6.30 ms** |
| roundTrip p95 | 623.64 ms | 496.07 ms | −127.57 ms |
| inference p50 | 477.25 ms | 471.27 ms | −5.98 ms |
| inference p95 | 621.74 ms | 495.45 ms | −126.29 ms |
| **boundary p50** | **0.39 ms** | **0.49 ms** | **+0.10 ms** |
| **boundary p95** | **1.52 ms** | **0.82 ms** | **−0.70 ms** |
| policy `push_samples` p50 | — | 0.14 ms | — (WASM call cost) |
| policy `push_samples` p95 | — | 0.27 ms | — |

The roundTrip/inference deltas are **negative** (rust-host was slightly faster
this run) — i.e. the per-chunk round-trip is dominated by inference, and
inference varies ~6 ms run-to-run. The *boundary* overhead — the only thing the
WASM boundary can affect — moves by **+0.10 ms p50**, two-to-three orders of
magnitude below the inference cost and well inside the noise.

### Realtime (8 ms-paced quanta, fire-and-forget) — sanity check, n = 120 each

| Metric | Baseline | Rust-host | Delta |
|---|---|---|---|
| roundTrip p50 | 466.28 ms | 466.94 ms | +0.66 ms |
| boundary p50 | 0.31 ms | 0.52 ms | +0.21 ms |
| roundTrip p95 | 2900.55 ms | 3046.73 ms | +146 ms |

The realtime p95 is large **on both paths** (worker-queue buildup + GC under the
fire-and-forget pattern, identical in shape on baseline and rust-host) — it is a
property of the scheduling pattern, not the Rust boundary; it cancels in the
comparison. The boundary p50 stays sub-millisecond on both.

Raw JSON: `results-baseline.json`, `results-rust-host.json`.
Screenshots: `evidence-baseline.png`, `evidence-rust-host.png`.

## Where the (tiny) cost goes, and whether transferables/SAB fix it

The gate asked: if the round-trip adds real latency, quantify exactly where
(postMessage serialization? wasm boundary? scheduling?) and whether
transferables/SharedArrayBuffer fix it. The honest answer is **it does not add
real latency**, but the breakdown of the ~0.4–0.5 ms boundary is:

1. **postMessage of the audio chunk — already zero-copy.** Both paths use a
   **transferable** (`postMessage(msg, [chunk.buffer])`). Verified empirically:
   after transfer the source `ArrayBuffer` detaches to `byteLength === 0`, so the
   64 KB (16000×f32) chunk is **moved, not structured-clone-serialized**. This is
   why the boundary is ~0.4 ms regardless of chunk size, and why making the chunk
   bigger (the 5 s production chunk is 5× this) does not change it. **No
   serialization cost to remove.**
2. **The WASM `push_samples` call — 0.14 ms p50.** This is the rust-host's only
   *added* step over the baseline. It runs once per 8 ms quantum; at 0.14 ms it
   consumes <2 % of a quantum and is invisible at chunk cadence. The command
   itself is a tiny `serde_wasm_bindgen` object (`{cmd, seq, len, dropped_silence}`)
   — cents on the dollar next to the 64 KB audio transfer.
3. **The command postMessage — not needed on the hot path.** In this spike the
   chunk PCM *is* the payload and the command is returned in-process from the
   WASM call (not posted separately), so there is no extra hop. The PRD's
   `JsHostEngine` can keep this shape: Rust decides in-process, then issues one
   `feed` postMessage carrying the chunk — exactly one hop, same as today.
4. **Scheduling.** Identical on both paths (same worker, same event loop, same
   pacing). The realtime p95 jitter confirms this: it appears equally on both.

**SharedArrayBuffer is available and enabled** (`crossOriginIsolated === true`,
COOP `same-origin` + COEP `credentialless`, served by the existing
`notetaker-server`), and it is what lets onnxruntime-web run multithreaded WASM —
but it is **not required to fix the boundary**, because the boundary is already
zero-copy via transferables and already sub-millisecond. SAB-backed audio rings
remain a *future* option for the AudioWorklet→main-thread hop (out of scope for
S2), not a fix for a problem that does not exist on the Rust→host hop.

## Implications for Phase I (Voxtral on the JsHostEngine)

- The two-cap recycle policy can live in Rust and drive the transformers.js host
  through `feed`/`generate` commands with **no streaming regression** from the
  boundary. The dominant cost is, and will remain, model inference inside the
  host — which is identical to today.
- Keep the **transferable** discipline for any audio/tensor payload crossing to
  the host (the spike confirms it is load-bearing for the flat boundary cost).
- The command object should stay small and serde-friendly; do **not** put audio
  PCM inside the serialized command — transfer it alongside, as both paths do.
- Policy decisions (`push_samples` here; recycle/feed/finalize in Phase I) at
  ~0.1–0.2 ms per call are free at chunk cadence. No need to batch or move them
  off-thread for latency reasons.

## Reproduction

```bash
# 1. Build the WASM policy core (wasm-pack, target web)
cd /Users/mike/dev/snt-spikes/b2-jshost/policy-core
cargo test --lib                                   # 4/4 native policy tests
wasm-pack build --release --target web --out-dir pkg

# 2. Serve the spike dir with COOP/COEP (reuses the repo's notetaker-server)
$CARGO_TARGET_DIR/release/notetaker-server /Users/mike/dev/snt-spikes/b2-jshost 8099
# (CARGO_TARGET_DIR is /Volumes/SSD/cargo-target in this env)

# 3. In a cross-origin-isolated browser, open each page and click "Run":
#    http://localhost:8099/baseline.html   → window.__RESULTS
#    http://localhost:8099/rust-host.html  → window.__RESULTS
```

Each run is 2 modes × 120 chunks × real Whisper-tiny inference (~5–6 min after
the model is cached).

## Caveats / honest limits

- Whisper-tiny.en (q4, WASM) is the measurement model per the task. The
  *boundary* delta is model-independent (it is postMessage + a fixed-cost WASM
  call, not inference), so the verdict generalizes; absolute inference numbers do
  not and are not the point.
- Measured on one machine (Apple Silicon, Chrome 148). The delta is so far inside
  noise that machine variance does not threaten the verdict, but the Firefox/
  Safari CPU-tier rows (PRD R1) are a separate, later acceptance.
- `realtime`-mode p95 is noisy by construction (fire-and-forget); the
  back-to-back number is the one to cite for the gate.
- This spike measures the **Rust→host** hop (the JsHostEngine boundary). The
  **AudioWorklet→main-thread** hop is unchanged from today and out of scope.
