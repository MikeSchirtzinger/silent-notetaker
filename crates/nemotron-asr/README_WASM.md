# nemotron-asr — wasm32 / browser backend (Phase 2)

The crate compiles to `wasm32-unknown-unknown` and exposes a `wasm-bindgen` API
(`WasmAsr`) that runs the Nemotron streaming ASR model in the browser via
[`ort-web`](https://ort.pyke.io/backends/web) (an `alternative-backend` for
`ort` that bridges to **onnxruntime-web**).

The native path (Phase 1) is untouched and still validated — see
`tests/golden.rs`. `audio.rs`, `vocab.rs`, and `streaming.rs` are unchanged;
the wasm path reuses the same mel front-end and detokenizer and mirrors the
same chunking + greedy RNN-T decode in async form (`src/backend_web.rs`).

## Build

```bash
# one-time:
cargo install wasm-pack         # if missing
rustup target add wasm32-unknown-unknown

# build the JS package into ./pkg
wasm-pack build --target web --out-dir pkg
```

This produces `pkg/`:

```
pkg/
├── nemotron_asr.js            # ES-module JS glue (default export = init())
├── nemotron_asr.d.ts          # TypeScript types for WasmAsr
├── nemotron_asr_bg.wasm       # our compiled wasm (~424 KB)
├── package.json
└── snippets/ort-web-*/        # ort-web's loader (fetches onnxruntime-web)
    ├── _loader.js
    └── _telemetry.js
```

## What the JS caller must provide

1. **Nothing for onnxruntime-web.** `ort-web` fetches the onnxruntime-web JS +
   `.wasm` itself at runtime (default origin `cdn.pyke.io`). You do *not* load
   onnxruntime-web yourself. If you enforce a Content-Security-Policy, allow
   `cdn.pyke.io` in `script-src` and `connect-src`. (Telemetry to
   `signal.pyke.io` is disabled by `WasmAsr::create`.)
2. **The three model artifacts as bytes** — your JS `fetch`es them and passes
   `Uint8Array`s in:
   - `encoder.onnx` (INT8: `MatMulInteger` + `DynamicQuantizeLinear`)
   - `decoder_joint_fp32.onnx` (FP32, standard `LSTM` — the wasm-safe decoder)
   - `tokenizer.model` (SentencePiece)
3. **16 kHz mono `Float32Array`** audio samples in `[-1, 1]`.

## JS usage

```js
import init, { WasmAsr } from './pkg/nemotron_asr.js';

// 1. Instantiate our wasm module.
await init();

// 2. Fetch the model artifacts (you host these; e.g. a CDN or /models).
const bytes = (u) => fetch(u).then(r => r.arrayBuffer()).then(b => new Uint8Array(b));
const [enc, dec, tok] = await Promise.all([
  bytes('/models/encoder.onnx'),
  bytes('/models/decoder_joint_fp32.onnx'),
  bytes('/models/tokenizer.model'),
]);

// 3. Create the engine. This is async: it inits ort-web (fetching
//    onnxruntime-web) and builds both ONNX sessions from the bytes.
const asr = await WasmAsr.create(enc, dec, tok);

// 4. Transcribe 16 kHz mono Float32 samples (offline; resets state first).
const samples = /* Float32Array, 16 kHz mono */;
const text = await asr.transcribe(samples);
console.log(text);

// Incremental streaming (carries encoder/decoder state across calls):
asr.reset();
for (const chunk of chunksOf(samples)) {
  const partial = await asr.transcribe_chunk(chunk); // text emitted this chunk
  if (partial) appendToUI(partial);
}
```

### API (`pkg/nemotron_asr.d.ts`)

```ts
class WasmAsr {
  static create(
    encoder_onnx: Uint8Array,
    decoder_onnx: Uint8Array,
    tokenizer_model: Uint8Array,
  ): Promise<WasmAsr>;
  reset(): void;
  transcribe(samples: Float32Array): Promise<string>;        // offline, resets first
  transcribe_chunk(samples: Float32Array): Promise<string>;  // incremental, keeps state
}
```

## Demo page

`examples/web/index.html` is a runnable demo. Serve the crate root over HTTP
(wasm + ES modules need `http://`, not `file://`):

```bash
wasm-pack build --target web --out-dir pkg
python3 -m http.server 8080
# open http://localhost:8080/examples/web/index.html
```

It fetches the model from `../../models/` and the sample clip from
`../../test-assets/test_16k.wav`.

## Notes / caveats

- **CPU build.** `WasmAsr::create` uses `ort_web::FEATURE_NONE` (CPU). The model
  runs comfortably realtime on CPU; WebGPU is actively worse for this model
  class (see `reference/FINDINGS.md`). To try WebGPU, switch the feature flag in
  `src/backend_web.rs` and add the `WebGPU` EP to the session builders.
- **Async, by necessity.** ort-web only supports `run_async`; session creation
  and output reads are async too. That is why the wasm path is a separate async
  driver rather than an impl of the synchronous `AsrBackend` trait.
- **In-browser numerical validation** (matching the golden transcript) is done
  with browser tooling, not `cargo test` — the Rust-side gate here is "native
  tests still green" + "`wasm-pack build` succeeds".
