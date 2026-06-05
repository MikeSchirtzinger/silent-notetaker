# R6 / K2 — Runtime vendoring decision (transformers.js + ort-web)

**Task:** z5-k2-vendoring (PRD Phase 7 / R6). **Date:** 2026-06-05. **Branch:** `rust-refactor`.

## Verdict: VENDOR EVERYTHING — all files fit under Cloudflare's 25 MiB/file limit.

The R6 evaluation target ("vendoring those runtimes shrinks `connect-src` to `self` +
Hugging Face + `ws://localhost:8765`") is **feasible in full**. Every runtime asset the
app fetches from a third-party CDN is under the 25 MiB (26,214,400-byte) Cloudflare Pages
per-file limit. There is **no partial-vendoring fallback needed** — the constraint the PRD
flagged ("the threaded/JSEP onnxruntime wasm is close to it") is real but does not bind:
the largest single file clears the limit with ~1.07 MiB of headroom.

## Witnessed size budget (downloaded + sha256-verified 2026-06-05)

All HTTP 200, all hashes pinned in `scripts/vendor-transformers.sh` + `scripts/vendor-ort-web.sh`:

| File | Bytes | MiB | Source | Used by |
|---|---:|---:|---|---|
| ort-web pyke 1.24.3: `ort-wasm-simd-threaded.wasm` | 12,361,745 | 11.79 | cdn.pyke.io | Nemotron + TitaNet (Rust `ort-web`) |
| ort-web pyke 1.24.3: `ort.wasm.min.js` | 50,067 | 0.05 | cdn.pyke.io | " |
| ort-web pyke 1.24.3: `ort-wasm-simd-threaded.mjs` | 24,274 | 0.02 | cdn.pyke.io | " |
| tfjs v3.8.1: `transformers.min.js` | 888,173 | 0.85 | jsdelivr | Whisper ×4 |
| tfjs v3.8.1: `ort-wasm-simd-threaded.jsep.wasm` | 21,596,019 | 20.60 | jsdelivr (v3 bundles ORT in own dist) | Whisper (webgpu) |
| tfjs v3.8.1: `ort-wasm-simd-threaded.jsep.mjs` | 44,484 | 0.04 | jsdelivr | " |
| tfjs v4.0.0-next.7: `transformers.min.js` | 537,903 | 0.51 | jsdelivr | Voxtral, Qwen, Moonshine, SenseVoice, Dual |
| ort 1.25: `ort-wasm-simd-threaded.asyncify.wasm` | 22,498,509 | 21.46 | jsdelivr | tfjs v4 device:'wasm' (Chrome/FF) |
| ort 1.25: `ort-wasm-simd-threaded.asyncify.mjs` | 47,387 | 0.05 | jsdelivr | " |
| ort 1.25: `ort-wasm-simd-threaded.jsep.wasm` | **25,096,522** | **23.93** | jsdelivr | tfjs v4 device:'webgpu' (Voxtral GPU) — **largest** |
| ort 1.25: `ort-wasm-simd-threaded.jsep.mjs` | 46,595 | 0.04 | jsdelivr | " |
| ort 1.25: `ort-wasm-simd-threaded.wasm` | 12,331,610 | 11.76 | jsdelivr | tfjs v4 Safari/WebKit |
| ort 1.25: `ort-wasm-simd-threaded.mjs` | 24,274 | 0.02 | jsdelivr | " |

- **Largest file:** `ort-wasm-simd-threaded.jsep.wasm` (ort 1.25) = 25,096,522 B = 23.93 MiB → fits (headroom 1,117,878 B = 1.07 MiB).
- **Total footprint:** ~91.1 MiB across 13 files (Cloudflare free tier: 20,000 files / unlimited total, so file count + total are not constraints).
- **Limit reconciliation:** Cloudflare's documented limit is **25 MiB = 26,214,400 bytes** (not decimal MB). This exactly matches `xtask deploy-gate`'s `CF_SIZE_LIMIT_BYTES = 25*1024*1024`, so the gate already enforces the correct threshold.

## Why the vendor set has multiple WASM variants (the non-obvious part)

`transformers.min.js` alone is NOT enough to go same-origin. transformers.js fetches its
onnxruntime-web WASM backend at runtime via `env.backends.onnx.wasm.wasmPaths`, and the
default points back at a CDN — so the WASM must be vendored too, and `wasmPaths` set to the
same-origin dir:

- **v3.8.1** bundles ORT in its *own* dist (`ort-wasm-simd-threaded.jsep.wasm`); default
  `wasmPaths = https://cdn.jsdelivr.net/npm/@huggingface/transformers@${version}/dist/`.
- **v4.0.0-next.7** fetches ORT from the *separate* `onnxruntime-web@1.25.0-dev.20260307-d626b568e0`
  package; default `wasmPaths = https://cdn.jsdelivr.net/npm/onnxruntime-web@${version}/dist/`
  and it **selects a different binary per browser/device**:
  - Chrome/Firefox `device:'wasm'` → `.asyncify.wasm`
  - any `device:'webgpu'` (Whisper auto, Voxtral) → `.jsep.wasm`
  - Safari/WebKit → plain `.wasm`

Because the app uses WebGPU (whisper-engine `device:'webgpu'` with `'auto'`+wasm fallback;
Voxtral GPU) AND must run on Safari/Firefox, the full cross-browser vendor set needs all
three ort-1.25 variants. Missing one silently breaks transcription on that browser/device
with no build error — which is exactly why the regression gate below cannot be skipped.

## CSP impact (the R6 privacy tightening)

After vendoring, the deploy `_headers` CSP drops `cdn.jsdelivr.net`, `unpkg.com`, and
`cdn.pyke.io` from BOTH `script-src` and `connect-src`, shrinking the egress allowlist to:

```
connect-src 'self' blob: data: https://huggingface.co https://*.hf.co
            https://cdn-lfs.huggingface.co https://cdn-lfs-us-1.huggingface.co
            ws://localhost:8765
script-src  'self' 'unsafe-inline' 'wasm-unsafe-eval' blob:
```

The only remaining cross-origin destinations are Hugging Face (model weights) and the
user's own localhost bridge. This is the extension-sandbox floor R6 names. **z2 made COEP
`require-corp`; same-origin vendored assets trivially satisfy it** (no CORS/CORP handshake
for same-origin), so vendoring also removes the last third-party fetch from the
cross-origin-isolated context.

## What this step EXECUTED (landed + validated, no browser needed)

1. `scripts/vendor-transformers.sh` — build-at-deploy fetch + sha256-verify of all 10
   transformers.js-side assets into `vendor/transformers-runtime/` (layout below).
   **Witnessed:** ran end-to-end, all 10 fetched HTTP 200, all hashes matched, all under
   25 MiB; cache/skip path verified on re-run. Binaries are NOT committed (gitignored).
2. `crates/nemotron-asr/src/backend_web.rs` — added `init_ort_web_vendored(base_url)` +
   `WasmAsr::create_with_dist(...)` (refactored `create` → `create`/`build`), mirroring the
   `silent-diarization` pattern. This is the one true Rust gap: Nemotron's ort-web had only
   the cdn.pyke.io default. **Witnessed:** wasm32 clippy `-D warnings` clean; native tests
   9/9 + golden pass.
3. `crates/silent-web/src/nemotron.rs` — `WasmNemotron::create_with_dist` (`js_name =
   createWithDist`) exposes the vendored path to `nemotron-engine.js`. **Witnessed:** wasm32
   clippy `-D warnings` clean.
4. `.gitignore` — `/vendor/transformers-runtime/` + `/vendor/ort-web-*/` so vendored
   binaries are never committed (hash-pinning, not committed blobs).

`silent-diarization` already shipped `create_with_dist` (B1/F1); TitaNet needs only the
loader wiring below.

## Vendored layout (produced by the scripts; copied to `dist/vendor/` at deploy)

```
dist/vendor/
  ort-web/1.24.3/            ort-wasm-simd-threaded.wasm  ort.wasm.min.js  ort-wasm-simd-threaded.mjs   (pyke; Nemotron+TitaNet)
  transformers/3.8.1/        transformers.min.js  ort-wasm-simd-threaded.jsep.{wasm,mjs}                (Whisper)
  transformers/4.0.0-next.7/ transformers.min.js                                                        (Voxtral/Qwen/…)
  onnxruntime-web/1.25/      ort-wasm-simd-threaded.{asyncify,jsep,}.{wasm,mjs}                          (tfjs v4 ORT)
```

## Remaining EXECUTE work — BROWSER-GATED, NOT done in this step (NEEDS-BROWSER-TEST)

These must land together and be validated end-to-end across Chrome (WebGPU + WASM), Firefox,
and WebKit (Playwright `test-playwright-cpu-tier.mjs`) with a real-mic gate + a fresh HF
model download + bridge + extensions. Applying any of them half-way (e.g. shrinking the CSP
before the loaders serve same-origin) breaks every engine, so they are deliberately deferred
to a browser-QA pass rather than landed blind.

1. **Loader path rewrites** (import URL + `wasmPaths`):
   - `index.html` L1698 (worker src) + L3466/L4495 (Voxtral): `import` from
     `./vendor/transformers/<ver>/transformers.min.js`; set
     `env.backends.onnx.wasm.wasmPaths = './vendor/onnxruntime-web/1.25/'` (v4) — v3's ORT
     lives next to its bundle so its default relative resolution already works once
     same-origin.
   - `whisper-engine.js` L79, `voxtral-engine.js` L43, `question-worker.js` L22: same
     import-URL swap + (v4) `wasmPaths`.
   - `nemotron-engine.js`: call `WasmNemotron.createWithDist(enc, dec, tok,
     './vendor/ort-web/1.24.3/')` when a vendored base is configured (keep `create` as the
     CDN rollback).
   - `diarization-engine.js`: call `WasmDiarization.create_with_dist(onnx, mel,
     './vendor/ort-web/1.24.3/')` (the Rust API already exists from F1).
   Recommend a single `window.__VENDOR_BASE` switch (default `./vendor/`) so a CDN rollback
   is one flag, preserving the strangler-fig escape hatch.

2. **`xtask gen_headers.rs`** — empty `STATIC_CDN_ORIGINS` (or drop it from `build_connect_src`
   + `build_csp`'s `script_cdns`). Flip the four tests that assert jsdelivr/unpkg/pyke
   PRESENT (`connect_src_includes_hf_and_cdn`, the doc-comment notes) to assert ABSENT.
   Then `cargo xtask gen-headers --out _headers` + commit the regenerated `_headers`.

3. **`deploy-cloudflare.sh`** — before the deploy-gate: run `scripts/vendor-ort-web.sh
   "$DIST/vendor/ort-web/1.24.3"` and `scripts/vendor-transformers.sh "$DIST/vendor"`; the
   files land same-origin in the bundle. deploy-gate already enforces the 25 MiB/file limit
   on them. The wasm-hashes manifest is unaffected (it hashes only the two app wasm crates).

4. **Regression (the acceptance bar):** every engine (Nemotron, TitaNet diarization, Whisper
   ×4, Voxtral, Qwen smart-questions, Moonshine, SenseVoice, Dual) transcribes with ZERO
   network requests to jsdelivr/unpkg/pyke (network-panel evidence); a fresh-profile HF model
   download still works; bridge (`ws://localhost:8765`) connects; extensions load; Firefox +
   WebKit via Playwright stay `crossOriginIsolated` and single-threaded-free under
   require-corp. Then `deploy-cloudflare.sh --dry-run` green.
