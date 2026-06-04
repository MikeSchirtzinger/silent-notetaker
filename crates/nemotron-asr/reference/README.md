# Reference materials — Nemotron ASR port provenance

The wasm/native streaming pipeline in this crate is a clean-room-ish port of the
offline + cache-aware streaming decode from:

- **Upstream reference:** https://github.com/altunenes/parakeet-rs (`MIT OR Apache-2.0`)
  — native Rust (`ort`) pipeline for NVIDIA Parakeet/Nemotron RNN-T models.
  See `src/backend_web.rs` and `src/streaming.rs` doc comments for what was
  mirrored (mel chunk layout, cache carry-forward, greedy RNN-T decode) and
  what diverges (async ort-web driver, rolling-audio-buffer mel recompute,
  edge-guarded live consumption).

Files here:

- `FINDINGS.md` — the 2026-06-02 browser de-risk: INT8-encoder + FP32-decoder
  combination chosen (the INT8 decoder uses a `com.microsoft` contrib LSTM op
  that onnxruntime-web may lack; FP32 decoder is plain `ai.onnx` LSTM), golden
  transcript, RTF measurements.
- `golden_harness.py` — Python mirror of the reference `transcribe_audio` /
  `decode_chunk` on onnxruntime (same kernels onnxruntime-web compiles); used
  to produce the golden outputs the Rust crate is tested against
  (`tests/golden.rs`, `test-assets/`).
- `inspect_onnx.py` — dumps the ONNX I/O names/shapes the port relies on
  (e.g. encoder output is `encoded`, cache tensor names).

A raw working-session dump (`parakeet_ref_source.txt`) is intentionally NOT
committed — it is an AI-assistant session transcript, not clean source. Clone
the upstream repo for the actual reference code.
