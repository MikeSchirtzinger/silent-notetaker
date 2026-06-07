# Nemotron streaming ASR — browser de-risk findings (2026-06-02)

**Verdict: PASS.** The B1 path (Nemotron INT8 ONNX in-browser via onnxruntime-web /
ort-web) is de-risked. The model, the INT8 quantization, and the cache-aware
streaming protocol all work and run comfortably in realtime on CPU.

## What was proven

Ran a Python golden harness (`golden_harness.py`) that mirrors `altunenes/parakeet-rs`
(`transcribe_audio` + `decode_chunk`) exactly, on onnxruntime-Python (same op kernels
onnxruntime-web compiles). Test audio: 6.03 s, `say`-synthesized, 16 kHz mono.

Ground truth: *"The quick brown fox jumps over the lazy dog. Artificial intelligence
is transforming the way we work and live."*

| Config | Transcript | RTF (CPU, 4 thr) |
|---|---|---|
| INT8 encoder + INT8 decoder | "The Quick Brown Fox jumps over the lazy dog, artificial intelligence is transforming the way we work and live." | 0.133× |
| **INT8 encoder + FP32 decoder** ⭐ | "the quick brown fox jumps over the lazy dog, artificial intelligence is transforming the way we work and live." | 0.139× |

- Word accuracy: 100% both configs. Only diff vs truth: a period rendered as comma.
- Streaming partials build incrementally across all 11 chunks → cache state carries correctly.
- RTF 0.13× = **7.5× faster than realtime on native CPU**. WASM is ~2–4× slower than
  native, so expect ~0.3–0.5× in-browser — still well under 1.0. CPU-realtime confirmed
  (no WebGPU needed; WebGPU is actively worse for this model class — see eval memory).

## The decoder op finding (why FP32 decoder)

- **encoder.onnx** (INT8, 881 MB): all `ai.onnx` opset-17. INT8 = `MatMulInteger` +
  `DynamicQuantizeLinear` (supported on onnxruntime-web WASM CPU). **No external `.data`
  file** → simple browser loading, and the ORT-web external-data+threads hang bug
  (#26858) does NOT apply.
- **decoder_joint.onnx** (INT8, 11 MB): uses `com.microsoft.DynamicQuantizeLSTM` — a
  *contrib* op that may be absent from onnxruntime-web's default WASM build. RISK.
- **decoder_joint_fp32.onnx** (36 MB, from altunenes): pure `ai.onnx` opset-17, standard
  `LSTM` ×2, no contrib ops → guaranteed in onnxruntime-web. Quantizing the decoder saved
  only ~25 MB anyway. **Ship INT8 encoder + FP32 decoder.**

## Confirmed model spec (ground truth from the ONNX graphs)

Encoder inputs: `processed_signal[1,128,T]`, `processed_signal_length[1]i64`,
`cache_last_channel[24,1,70,1024]`, `cache_last_time[24,1,1024,8]`, `cache_last_channel_len[1]i64`.
Encoder outputs: `encoded[1,1024,T_enc]` (name confirmed — not `outputs`), `encoded_len`,
+ the three `*_next` caches.
Decoder inputs: `encoder_outputs[1,1024,1]`, `targets[1,1]i32`, `target_length[1]i32`,
`input_states_1/2[2,1,640]`. Outputs: `outputs[..,1025]`, `prednet_lengths`, `output_states_1/2`.

Mel front-end (Nemotron, NO normalization): 16 kHz, preemph 0.97, n_fft 512, win 400 (symmetric
Hann), hop 160, 128 Slaney mels, **power** spectrum, `ln(x + 2^-24)`. Chunk = 56 mel frames +
9 pre-encode lookback (560 ms). Greedy RNN-T: max 10 symbols/frame, blank=1024, LSTM state
+ last_token carried across the whole utterance.

## Files

- `golden_harness.py` — the reference impl. Run: `./venv/bin/python golden_harness.py test_16k.wav [decoder.onnx]`
- `inspect_onnx.py` — dumps tensor I/O + op histogram + risk flags.
- `parakeet_ref_dump.txt` — verbatim parakeet-rs source used as the porting spec.
- `models/` (gitignored) — encoder.onnx (881M), decoder_joint.onnx (INT8 11M),
  decoder_joint_fp32.onnx (36M), tokenizer.model.

## Next steps toward a real browser engine

1. Port `golden_harness.py` to the target: either (B1) `parakeet-rs` fork swapping
   `ort` → `ort-web`, or a thin onnxruntime-web JS engine cloned from the existing
   TitaNet engine in `index.html`. The golden transcript above is the validation oracle.
2. Adapt the chunk manager to live mic audio (AudioWorklet ring buffer already exists).
3. Wire into the `index.html` engine switch (alongside `startSenseVoice()` etc.).
4. Set up cross-origin isolation (Cloudflare Pages `_headers`) if multi-threaded WASM is wanted;
   single-thread is likely fine given the RTF headroom.
5. Consider INT4 encoder (lokkju) to cut the 881 MB download if accuracy holds.


---

## ERRATUM (2026-06-05): ship the INT8 decoder, NOT the fp32 one

The "ship FP32 decoder" recommendation above is **superseded**. Empirical A/B
(`dev/ab-test/` in the app repo) showed:

1. `DynamicQuantizeLSTM` **is supported** by onnxruntime-web's WASM build —
   the INT8 `decoder_joint.onnx` loads and decodes in-browser (the "RISK"
   flagged above never materialized).
2. `decoder_joint_fp32.onnx` (from altunenes/parakeet-rs) is a **mismatched
   checkpoint**, not an fp32 export of the lokkju INT8 model: it emits
   spelled-out numbers (no inverse text normalization) and garbles token-dense
   audio (~20% WER vs ~9% for the INT8 decoder on the same number-heavy clip;
   reproduced identically on native ort, onnxruntime-Python, and ort-web).

All three artifacts (encoder, INT8 decoder, tokenizer) come from
`lokkju/nemotron-speech-streaming-en-0.6b-int8 @ 95df6c8` — single-source.
