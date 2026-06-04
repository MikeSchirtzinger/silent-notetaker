# Spike B3: CI browser-wasm tests with vendored ort-web assets

**Status:** COMPLETED (browser_smoke suite), NEEDS-BROWSER-TEST (WasmAsr full round-trip)
**Date:** 2026-06-04
**Agent:** b3-spike-ci (engineer/sonnet)

---

## Summary

`wasm-pack test --headless --chrome -- --test browser_smoke` runs 4 tests
locally with **zero network fetches** to `cdn.pyke.io` or any CDN.  Proven by
blocking `cdn.pyke.io` in Chrome's host resolver during the run (see
Validation section).

The ORT-web runtime assets are identified, downloaded, hashed, and documented.
A draft GH Actions workflow lives in `snt-spikes/b3-ci/.github/workflows/`.
The full WasmAsr golden round-trip (calling `WasmAsr::create()`) is blocked on
the model fixture + vendor-server same-origin problem documented below; it is
marked `NEEDS-BROWSER-TEST`, not faked.

---

## 1. Asset identification

ort-web 0.2.1 (the version used by nemotron-asr) wraps **onnxruntime-web 1.24**.
Its `_loader.js` (compiled into the wasm snippet via `wasm-bindgen`) loads three
assets from `https://cdn.pyke.io/0/pyke:ort-rs/web@1.24.3/` for `FEATURE_NONE`
(CPU-only, the mode nemotron-asr uses):

| File | Purpose | Size |
|---|---|---|
| `ort.wasm.min.js` | Main ORT-web entrypoint (injected `<script>`) | 50 KB |
| `ort-wasm-simd-threaded.wasm` | ONNX Runtime WASM binary (SIMD+threads) | 12 MB |
| `ort-wasm-simd-threaded.mjs` | Emscripten wrapper module | 24 KB |

### How the fetch happens

1. Rust code calls `ort_web::api(FEATURE_NONE).await`.
2. That calls `binding::init_runtime(0, JsValue::null()).await`.
3. In JS: `initRuntime(0, null)` in `_loader.js` → `dist = DEFAULT_DIST[0]`
   → constructs a `<script>` tag pointing to
   `https://cdn.pyke.io/0/pyke:ort-rs/web@1.24.3/ort.wasm.min.js`.
4. `ort.wasm.min.js` then fetches `ort-wasm-simd-threaded.mjs` and
   `ort-wasm-simd-threaded.wasm`.

The fetch is triggered **only when `ort_web::api(...)` is called** — not at
module import time. The `initRuntime` symbol is imported at ES-module load time
(present in the generated JS glue), but the fetch fires only when invoked from Rust.

### Vendoring mechanism

`ort-web::Dist::new(base_url)` lets Rust code redirect the loader to any base
URL instead of `cdn.pyke.io`:

```rust
// Instead of:
ort_web::api(FEATURE_NONE).await

// Use a local server:
ort_web::api(ort_web::Dist::new("http://localhost:19999/").with_binary_name("ort-wasm-simd-threaded.wasm")).await
```

This passes the custom `dist` object through to `initRuntime(features, dist)` in
`_loader.js`, which then constructs `<script>` and `<link rel="preload">` elements
pointing to the local URL.

---

## 2. Vendored asset manifest

Downloaded from `cdn.pyke.io` 2026-06-04 and verified:

| File | Size | SHA-256 |
|---|---|---|
| `ort.wasm.min.js` | 50 KB | `4043d2deda6a2e2fc783afc2b06d984068808181b88d451862c1230c433fce7a` |
| `ort-wasm-simd-threaded.wasm` | 12 MB | `be0e129949062ad50290ef94683fac8be5bb6156f709e030b7a5f1661a2f6c17` |
| `ort-wasm-simd-threaded.mjs` | 24 KB | `5687566b1bc1c8cf628d76c2ddb16b2a3b81a7997273d4666564880495088e57` |

The SRI hashes embedded in `_loader.js` itself (using SHA-384, Base64-encoded):

```
ort.wasm.min.js: sha384-1SBQgvQsxJRGAOAJ6K2nPaLO1SKelZwoF+biXgv2/D9fPspYLhvG4WIMDb/BUoJC
ort-wasm-simd-threaded.mjs: sha384-/xM/eq8aUBJZgBuVwTQcLA5KlNmP6HOaENdJVgCkA/06cOMdL9EIQtmMuXOlMZEd
ort-wasm-simd-threaded.wasm: sha384-sZw0EVBgUn+dNhQfjHDg8lwtmicKMm1bTvWS4rIRNxoVN1S9HkVyJ2nreMpYruEZ
```

These match `ort-web 0.2.1+1.24` which ships `onnxruntime-web@1.24.3`.

**Cloudflare Pages 25 MB/file limit:** `ort-wasm-simd-threaded.wasm` is 12 MB — well
within the limit.  The threaded+JSEP build (for WebGPU) is ~22 MB; also within limit
but not used in the CPU path.

---

## 3. Changes to the repo

### `nemotron-asr/tests/golden.rs` (MODIFIED)

Added `#![cfg(not(target_arch = "wasm32"))]` inner attribute.  The native golden
test references `nemotron_asr::Nemotron` (an alias for
`StreamingAsr<OrtBackend>`) which is `#[cfg(not(target_arch = "wasm32"))]` gated
in `lib.rs`.  Without this gate, `cargo build --tests --target wasm32-unknown-unknown`
fails with `E0432`.

### `nemotron-asr/tests/browser_smoke.rs` (NEW)

Four `#[wasm_bindgen_test]` tests exercising the pure-Rust mel frontend and
vocab constants.  No calls to `WasmAsr::create()` or `init_ort_web()` — zero ORT
runtime assets required and zero CDN fetches possible.

```
test mel_frontend_constructs ... ok
test mel_silence_shape_correct ... ok
test mel_non_silence_has_energy ... ok
test vocab_decode_single_blank_is_blank ... ok
```

### `nemotron-asr/Cargo.toml` (MODIFIED)

- Added `wasm-bindgen-test = "0.3"` to `[dev-dependencies]`.
- Added explicit `[[test]]` entries for `golden` and `browser_smoke` so cargo
  builds each only for its correct target.

### `nemotron-asr/webdriver.json` (NEW)

Chrome flags for local testing:
```json
{
  "goog:chromeOptions": {
    "args": ["--disable-dev-shm-usage", "--no-sandbox", "--disable-gpu", "--headless=new"]
  }
}
```

---

## 4. Local run procedure (exact commands)

```bash
# From repo root on the rust-refactor branch:

# Step 1: Vendor the ORT-web assets (cached after first run)
snt-spikes/b3-ci/vendor-ort-web.sh nemotron-asr/vendor/ort-web-1.24.3

# Step 2: Run the tests with CDN blocked (proves zero CDN fetches)
CHROMEDRIVER=/path/to/chromedriver-148 \
  wasm-pack test --headless --chrome \
  --chromedriver /path/to/chromedriver-148 \
  nemotron-asr -- --test browser_smoke

# Or use the convenience script (also starts the vendor server for future tests):
CHROMEDRIVER=/path/to/chromedriver-148 snt-spikes/b3-ci/run-browser-tests-local.sh
```

**ChromeDriver version pin:** ChromeDriver major version must match Chrome major
version.  Download from:
`https://googlechromelabs.github.io/chrome-for-testing/`

---

## 5. Validation output

### Local run (2026-06-04 on Apple Silicon, Chrome 148, ChromeDriver 148)

```
wasm-pack test --headless --chrome --chromedriver .../chromedriver-148 -- --test browser_smoke

Running headless tests in Chrome on http://127.0.0.1:56788/
Try find `webdriver.json` for configure browser's capabilities:
Ok
Loading Wasm module...

running 4 tests
test vocab_decode_single_blank_is_blank ... ok
test mel_silence_shape_correct ... ok
test mel_non_silence_has_energy ... ok
test mel_frontend_constructs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 filtered out; finished in 1.90s
```

### CDN-blocked proof (cdn.pyke.io → 127.0.0.2)

Run with `--host-resolver-rules=MAP cdn.pyke.io 127.0.0.2,MAP signal.pyke.io 127.0.0.2`:

```
Running headless tests in Chrome on http://127.0.0.1:56939/
Try find `webdriver.json` for configure browser's capabilities:
Ok
Loading Wasm module...

running 4 tests
test vocab_decode_single_blank_is_blank ... ok
test mel_silence_shape_correct ... ok
test mel_non_silence_has_energy ... ok
test mel_frontend_constructs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 filtered out; finished in 1.94s
```

All 4 tests pass with both `cdn.pyke.io` and `signal.pyke.io` explicitly
unreachable.  Any attempt to call `init_ort_web()` / `initRuntime()` would cause
the `<script>` tag injection to fail silently or the wasm fetch to timeout → the
test suite would hang and eventually fail.  It does not hang → proven zero CDN
fetches.

---

## 6. WasmAsr full round-trip (NEEDS-BROWSER-TEST)

**Status:** NOT COMPLETED — marked NEEDS-BROWSER-TEST, not faked.

Calling `WasmAsr::create(encoder_bytes, decoder_bytes, tokenizer_bytes)` requires:

1. **Model bytes**: the 900 MB Nemotron model files (not in repo, not in CI cache yet).
2. **ORT runtime**: `init_ort_web()` must succeed, which means the vendor server
   must be reachable from the browser context on the same origin as the
   wasm-bindgen-test server.

### Same-origin problem

The wasm-bindgen-test-runner starts its server on a random port (e.g., `127.0.0.1:56788`).
A separate vendor server on port `19999` is a different origin.  Chrome allows
cross-origin fetches from `<script>` tags, but the ORT runtime's Emscripten
module uses `SharedArrayBuffer` (for SIMD threading), which requires
Cross-Origin Isolation (`COOP: same-origin` + `COEP: credentialless`).  The
wasm-bindgen-test-runner does set these headers on its own origin, but the
vendor server does not share that origin → loading ORT from a different port is
blocked by COEP.

### Resolution path (Phase 1, Task D3/D4)

Option A (recommended): Serve vendor assets at the same origin as the test server.
The wasm-bindgen-test-runner serves the entire directory containing the test wasm
binary (`target/wasm32-unknown-unknown/debug/deps/`).  A `build.rs` script can
copy vendor assets to `OUT_DIR` and a symlink/copy into the deps dir would let
the test reference them as `./vendor/ort-web-1.24.3/`.  Needs investigation of
whether `OUT_DIR` is served.

Option B: Use `WASM_BINDGEN_TEST_NO_ORIGIN_ISOLATION=1` to disable COEP on the
test server, then serve vendor assets from any localhost port.  This disables
`SharedArrayBuffer` — but the CPU-only ORT build (FEATURE_NONE,
`ort-wasm-simd-threaded.wasm`) does use threads internally.  Risk: ORT silently
falls back to single-thread mode.  Needs measurement.

Option C: Upstream contribution to wasm-bindgen-test-runner: add a
`--static-dir` flag or `WASM_BINDGEN_TEST_STATIC_DIR` env var to serve extra
directories from the test server root.

Option D: In the `browser_smoke.rs` WasmAsr test, instead of `init_ort_web()`,
patch `window.ort` before the wasm code runs by injecting a pre-loaded ORT
instance.  Complex without control over the test HTML.

**For Phase 1 (Task D3/D4):** Use Option B as the unblock — disable COEP,
measure thread fallback impact on test correctness, document the tradeoff.
Option A (build.rs copy) is the correct long-term fix.

---

## 7. GH Actions workflow (draft)

`snt-spikes/b3-ci/.github/workflows/wasm-browser-tests.yml`

Key design:
- `native` job: fmt/check/clippy/test (skips golden if models absent).
- `browser-smoke` job: downloads vendored assets, starts vendor server on port
  19999, blocks CDN in Chrome, runs `wasm-pack test --headless --chrome`.
- `browser-wasm-golden` job: **commented out** — activated once NEEDS-BROWSER-TEST
  gates clear and the model cache / registry pin is in place (Task D1 + D3).

Cache strategy:
- Cargo registry + target dir: keyed by `Cargo.lock`.
- ort-web vendor assets: keyed by `ORT_WEB_VERSION` string — stable across runs,
  no re-download unless the version changes.
- Nemotron model files (for future golden job): keyed by registry revision SHA.

ChromeDriver version management: `wasm-pack` auto-downloads chromedriver; on
GitHub-hosted runners (Ubuntu) Chrome and ChromeDriver versions align.
If the runner is updated and the version drifts, pin with
`--chromedriver /path/to/matching-chromedriver`.

---

## 8. Spike files

| Path | Purpose |
|---|---|
| `snt-spikes/b3-ci/.github/workflows/wasm-browser-tests.yml` | Draft GH Actions workflow |
| `snt-spikes/b3-ci/vendor-ort-web.sh` | Download + verify ort-web assets |
| `snt-spikes/b3-ci/run-browser-tests-local.sh` | Repeatable local run script |
| `snt-spikes/b3-ci/vendor/ort-web-1.24.3/` | Vendored assets (not committed to repo) |
| `nemotron-asr/tests/browser_smoke.rs` | New browser test file |
| `nemotron-asr/tests/golden.rs` | Added `#![cfg(not(target_arch = "wasm32"))]` |
| `nemotron-asr/Cargo.toml` | Added wasm-bindgen-test dep + explicit [[test]] entries |
| `nemotron-asr/webdriver.json` | Chrome flags for local headless testing |

**NOT committed:** `snt-spikes/b3-ci/vendor/ort-web-1.24.3/` (binary assets,
~12 MB — add to `.gitignore` or download in CI).  Add `nemotron-asr/vendor/` to
`.gitignore` as well.

---

## 9. Handoff to D3/D4

Task D3 (chunking consolidation) requires the browser wasm golden test to pass.
The harness exists now (`browser_smoke.rs`) but the WasmAsr section is
`NEEDS-BROWSER-TEST`.  D3 / D4 agents should:

1. Resolve the same-origin problem (Option B first, then A).
2. Add a `wasm_golden` integration test in `nemotron-asr/tests/` that runs
   `WasmAsr::transcribe()` on the same `test-assets/test_16k.wav` fixture
   and checks the transcript against `test-assets/golden_transcript.txt`.
3. Update the GH Actions workflow to restore the model cache and run `wasm_golden`.
4. Remove the `NEEDS-BROWSER-TEST` annotation from `browser_smoke.rs`.
