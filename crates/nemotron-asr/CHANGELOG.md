# Changelog

All notable changes to `nemotron-asr` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

MSRV (Rust 1.93) is part of the public contract: an MSRV bump is a
semver-minor change. API stability is enforced by `cargo-semver-checks`.

## [Unreleased]

First public release preparation. Reconstructed from the crate's git history.

### Added

- **Native streaming ASR engine** for NVIDIA
  `nemotron-speech-streaming-en-0.6b` (cache-aware RNN-T): pure-Rust log-mel
  front-end (`audio`), wasm-safe SentencePiece detokenizer (`vocab`), the
  `AsrBackend` trait with the native `OrtBackend` over [`ort`], and the
  `StreamingAsr` cache-aware decode loop. Validated against a Python golden
  harness at 100% word accuracy, RTF 0.139× on native CPU. (2026-06-02)
- **`wasm32` / browser backend** (`backend_web::WasmAsr`) over
  [`ort-web`](https://crates.io/crates/ort-web) (onnxruntime-web), exposed via
  `wasm-bindgen`. Reuses the unchanged native mel front-end and detokenizer;
  inference is async because `ort-web` is async-only. Includes a runnable demo
  page (`examples/web/index.html`). Wasm binary ~424 KB. (2026-06-02)
- **Incremental streaming API**: `WasmAsr::transcribe_chunk` / `reset` and
  `StreamingAsr` chunk feeding, carrying encoder and decoder state across
  chunks (560 ms chunk = 56 mel frames + 9 lookback). (2026-06-02)
- **Reference provenance**: `reference/FINDINGS.md` (the browser de-risk
  verdict, op-level INT8/FP32 decoder analysis, and the CPU-vs-WebGPU
  finding), the Python golden harness, and the ONNX inspector. (2026-06-04)
- **Browser-wasm test scaffolding** (`tests/browser_smoke.rs`) runnable under
  `wasm-pack test --headless --chrome` with vendored onnxruntime-web runtime
  assets, so CI does not depend on `cdn.pyke.io`. (2026-06-04)
- Publishing metadata, dual `LICENSE-MIT` / `LICENSE-APACHE`, this changelog,
  and the crate README with the measured benchmark table.

### Changed

- **Perceived latency halved** via an edge-guard fix: the streaming loop no
  longer decodes mel frames in the STFT's right-edge zero-padding zone
  (`EDGE_GUARD_FRAMES`). With 250 ms feed buffers and 8 onnxruntime-web WASM
  threads on a cross-origin-isolated page, time-to-first-text dropped to
  ~0.65 s and browser RTF improved from 0.40× to 0.28×. A warm-up chunk at
  load pays JIT tier-up before the user's first words. (2026-06-04)
- **One chunking core**: the previously duplicated mel-chunk construction and
  greedy-decode helpers in `streaming.rs` and `backend_web.rs` were
  consolidated into a single `chunk_core` module. Both backends now share one
  mel-chunk layout policy and one `argmax`; the golden transcript is
  byte-identical across this change. (2026-06-04)
- Opted into the workspace lint bar (`clippy` pedantic, `unsafe_code`
  forbidden, `unwrap_used` denied in production paths); eliminated all `s![]`
  macro usage in favor of `index_axis` to resolve the `unsafe_code` conflict.
  Every per-site `#[allow]` carries a documented rationale. (2026-06-04)

### Fixed

- Crate-level docs now build clean for docs.rs at the published bar (zero
  warnings with `missing_docs` denied): native-only intra-doc links to the
  wasm `backend_web` module are rendered as plain code spans so the native
  documentation target resolves without broken links.

[Unreleased]: https://github.com/brevity-ventures/silent-notetaker/commits/rust-refactor
[`ort`]: https://crates.io/crates/ort
