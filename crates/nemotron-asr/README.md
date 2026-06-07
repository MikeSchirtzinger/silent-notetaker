# nemotron-asr

[![Crates.io](https://img.shields.io/crates/v/nemotron-asr.svg)](https://crates.io/crates/nemotron-asr)
[![Docs.rs](https://docs.rs/nemotron-asr/badge.svg)](https://docs.rs/nemotron-asr)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**Cache-aware RNN-T streaming speech recognition for NVIDIA
[`nemotron-speech-streaming-en-0.6b`](https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b)
— the same Rust code runs natively and in the browser (`wasm32`).**

`nemotron-asr` is a small, dependency-light ASR engine: a pure-Rust log-mel
front-end and SentencePiece detokenizer wrapped around an INT8 ONNX encoder and
an FP32 RNN-T decoder/joint, with a cache-aware streaming loop that carries
encoder and decoder state across chunks. Inference runs through
[`ort`](https://crates.io/crates/ort) (ONNX Runtime) natively, and through
[`ort-web`](https://crates.io/crates/ort-web) (onnxruntime-web) in the browser
— **no rewrite, one chunking core, one decode policy.**

The wasm binary is ~424 KB. It runs comfortably faster than realtime on CPU; no
WebGPU required (see [Why CPU, not WebGPU](#why-cpu-not-webgpu)).

---

## Measured benchmarks

All numbers below are measured, not projected. The native figures come from the
Python golden harness that mirrors the Rust decode loop op-for-op
(`reference/golden_harness.py`); the browser figures come from real in-app runs
against the same golden clip. Test clip: 6.03 s, 16 kHz mono, INT8 encoder +
FP32 decoder. Sources: [`reference/FINDINGS.md`](reference/FINDINGS.md) and the
project's `docs/research/` evidence trail (browser-validated 2026-06-04).

| Metric | Native (CPU) | Browser (wasm, CPU) |
|---|---|---|
| Real-time factor (RTF) | **0.139×** (≈7× faster than realtime) | **0.28–0.5×** |
| Time-to-first-text (TTFT) | — | **~0.65 s** (after the edge-guard fix) |
| Word accuracy on golden clip | **100%** | **100%** |

Notes on the browser numbers:

- **TTFT ~0.65 s** is the perceived time-to-first-text in the real app at 250 ms
  feed buffers, after an edge-guard fix that stopped the streaming loop from
  decoding mel frames in the STFT's right-edge zero-padding zone.
- **RTF 0.28–0.5×**: the 0.28× end is with 8 onnxruntime-web WASM threads on a
  cross-origin-isolated page; 0.40× with 4 threads. WASM is ~2–4× slower than
  native, which is why native lands at 0.139× and the browser at 0.28–0.5× — all
  well under the 1.0× realtime ceiling.
- **100% word accuracy** on the golden clip in both backends; the only delta vs
  ground truth is a period rendered as a comma.

The crate's chunking + greedy RNN-T decode logic was later **consolidated behind
a single `chunk_core` module** so the native and wasm backends share one
mel-chunk layout policy and one `argmax` — eliminating the risk of silent
correctness drift between the two paths. The golden transcript is byte-identical
across that consolidation.

### Why CPU, not WebGPU

For this model class — a small INT8 encoder with irregular ops — CPU
consistently beat WebGPU in measurement. The streaming loop ships CPU-only by
default. The full op-level analysis (and the FP32-decoder decision) is in
[`reference/FINDINGS.md`](reference/FINDINGS.md).

---

## Quickstart

### Native

```toml
[dependencies]
nemotron-asr = "0.1"
```

```rust,no_run
use nemotron_asr::Nemotron;

// `models/` holds encoder.onnx, decoder_joint.onnx, tokenizer.model.
let mut asr = Nemotron::from_pretrained("models")?;
let audio = nemotron_asr::audio::load_wav_mono("test-assets/test_16k.wav")?;
let transcript = asr.transcribe_audio(&audio)?;
println!("{transcript}");
# Ok::<(), nemotron_asr::Error>(())
```

Run the bundled example end-to-end:

```bash
cargo run --release --example transcribe -- models test-assets/test_16k.wav
```

The native `ort` dependency uses `download-binaries`, so the ONNX Runtime
shared library is fetched automatically on first build.

### Browser (wasm32 / `ort-web`)

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack          # if missing
wasm-pack build --target web --out-dir pkg
```

```js
import init, { WasmAsr } from './pkg/nemotron_asr.js';

await init();

// Your JS fetches the three model artifacts and passes them as Uint8Arrays.
const bytes = (u) => fetch(u).then(r => r.arrayBuffer()).then(b => new Uint8Array(b));
const [enc, dec, tok] = await Promise.all([
  bytes('/models/encoder.onnx'),
  bytes('/models/decoder_joint.onnx'),
  bytes('/models/tokenizer.model'),
]);

const asr = await WasmAsr.create(enc, dec, tok);

// Offline transcription of 16 kHz mono Float32 samples:
const text = await asr.transcribe(samples);

// Incremental streaming (carries encoder/decoder state across calls):
asr.reset();
for (const chunk of chunksOf(samples)) {
  const partial = await asr.transcribe_chunk(chunk);
  if (partial) appendToUI(partial);
}
```

`ort-web` fetches the onnxruntime-web runtime itself at load time — you don't
load onnxruntime-web yourself. A runnable demo page lives at
`examples/web/index.html`; full browser setup (CSP, threading, COOP/COEP) is in
[`README_WASM.md`](README_WASM.md).

---

## Model artifacts

This crate ships **code, not weights.** It expects three artifacts, fetched by
you (e.g. from a pinned Hugging Face revision or your own CDN):

| File | Format | Approx. size |
|---|---|---|
| `encoder.onnx` | INT8 (`MatMulInteger` + `DynamicQuantizeLinear`) | ~881 MB |
| `decoder_joint.onnx` | INT8 (`DynamicQuantizeLSTM` — supported by native ORT *and* onnxruntime-web) | ~11 MB |
| `tokenizer.model` | SentencePiece | ~251 KB |

The model is NVIDIA's `nemotron-speech-streaming-en-0.6b`, distributed under the
**NVIDIA Open Model License** (redistribution permitted with attribution). This
crate's *source code* is MIT OR Apache-2.0 and does not redistribute the model.

---

## How it works

- **`audio`** — log-mel front-end: pre-emphasis 0.97, `n_fft` 512, win 400
  (symmetric Hann), hop 160, 128 Slaney mels, power spectrum, `ln(x + 2^-24)`,
  no normalization. Pure Rust, wasm-safe.
- **`vocab`** — pure-Rust SentencePiece detokenizer (no native C deps).
- **`model`** — the `AsrBackend` trait and the native `OrtBackend` (the only
  place that links `ort`).
- **`chunk_core`** — the one shared mel-chunk builder + `argmax`, used by both
  backends so chunking policy lives in a single auditable place.
- **`streaming`** — the cache-aware RNN-T decode loop (`StreamingAsr`): max 10
  symbols/frame, blank = 1024, LSTM state + last token carried across the whole
  utterance; encoder caches carried across chunks (560 ms chunk = 56 mel frames
  + 9 lookback).

The wasm backend (`backend_web`, `target_arch = "wasm32"` only) mirrors the same
chunking and greedy RNN-T decode in async form, because `ort-web` is async-only.

---

## Versioning

Semantic versioning, with API stability enforced by
[`cargo-semver-checks`](https://crates.io/crates/cargo-semver-checks). MSRV is
**Rust 1.93** and is treated as part of the public contract: MSRV bumps are
semver-minor. See [`CHANGELOG.md`](CHANGELOG.md).

---

## Where this comes from

`nemotron-asr` is the flagship open-source crate from **Brevity Ventures**. It
is the proven Rust/WASM inference foundation behind **Silent Notetaker** — a
fully client-side, "private by architecture" meeting note-taker where audio
never leaves the browser.

- **Silent Notetaker** (the proof): https://github.com/brevity-ventures/silent-notetaker
- **Brevity Ventures** (the team): https://brevity.ventures

The crate is the ad; the app is the proof.

---

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
