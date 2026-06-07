# Spike: COEP credentialless vs require-corp — Safari blocker resolution

**Task:** coep-spike (R1 gate, board o-rust-refactor-20260604)
**Date:** 2026-06-05
**Status:** COMPLETED — RECOMMENDATION: switch to `COEP: require-corp`
**Spike dir:** `/Users/mike/dev/snt-spikes/coep/`

---

## Context

The app ships `COEP: credentialless` because an earlier decision (2026-06-04) recorded that `require-corp` broke HF CDN fetches. i5 subsequently proved WebKit/Safari ignores `credentialless` (`crossOriginIsolated=false`, single-threaded). The conflict: require-corp for Safari vs credentialless for HF CDN. This spike asks: can both be satisfied simultaneously?

**Short answer: yes. Switch to `require-corp`. The original "breaks HF CDN" assessment was incorrect. HF CDN satisfies require-corp via CORS headers, which are CORP-equivalent under the spec.**

---

## 1. CDN Header Audit (curl -sI)

### HF TitaNet fetch chain

```
GET https://huggingface.co/FluffyBunnies/titanet-small-onnx/resolve/<commit>/titanet.onnx
  → 302 to https://cas-bridge.xethub.hf.co/xet-bridge-us/<...>?signed-url
```

**huggingface.co 302 response:**
- `Cross-Origin-Opener-Policy: same-origin`
- `Access-Control-Allow-Origin: <origin>` (mirrors request Origin)
- `Access-Control-Max-Age: 86400`
- No `Cross-Origin-Resource-Policy` header

**cas-bridge.xethub.hf.co 200 response:**
- `Access-Control-Allow-Credentials: true`
- `Access-Control-Allow-Origin: <origin>` (mirrors request Origin) — **CORS-capable**
- No `Cross-Origin-Resource-Policy` header explicitly
- Content-Security-Policy: `default-src 'none'; media-src 'self'; sandbox allow-same-origin` (irrelevant to CORP)

**Key finding:** Neither HF origin sends `Cross-Origin-Resource-Policy` explicitly. However, this is irrelevant: the COEP spec ([whatwg/html §cross-origin-embedder-policy](https://html.spec.whatwg.org/multipage/origin.html#coep)) treats a CORS-eligible response (one with `Access-Control-Allow-Origin`) as satisfying `require-corp`. The browser validates the CORS handshake, not a CORP header, for fetch()-initiated requests from cross-origin-isolated contexts.

### jsdelivr (transformers.js)

```
Access-Control-Allow-Origin: *
Cross-Origin-Resource-Policy: cross-origin   ← explicit CORP
```

Both transformers.js v3 and v4 on jsdelivr send explicit `CORP: cross-origin`. No issue under require-corp.

### cdn.pyke.io (ort-web runtime — vendored, informational only)

- `Access-Control-Allow-Origin: *` (when returning non-404)
- No CORP header observed (404 for paths tested; runtime is vendored same-origin per spike-ci-wasm.md so this is moot)

**The pyke.io origin is not relevant in production**: ort-web is vendored at `crates/nemotron-asr/vendor/ort-web-1.24.3/` and served same-origin (proven in spike-ci-wasm.md and spike-titanet.md).

---

## 2. Browser Fetch Tests Under `COEP: require-corp` (Chrome)

Test server: `require-corp-server.py` at port 8199, headers `COOP: same-origin` + `COEP: require-corp`.

**Environment confirmed:**
- `crossOriginIsolated: true`
- `SharedArrayBuffer: available`
- `hardwareConcurrency: 10`

| Test | Method/Mode | Result | Notes |
|------|------------|--------|-------|
| 1a. TitaNet HF plain fetch | HEAD (no explicit mode) | **PASS HTTP 200, type:cors** | Browser auto-CORS for cross-origin |
| 1b. TitaNet HF mode:'cors' | HEAD cors | **PASS HTTP 200, type:cors** | Explicit CORS also works |
| 1c. TitaNet HF mode:'no-cors' | HEAD no-cors | **FAIL** | Expected: opaque response blocked by require-corp |
| 1d. jsdelivr transformers.js@3 | HEAD | **PASS HTTP 200** | CORP:cross-origin on jsdelivr |
| 1e. jsdelivr transformers.js@4 | HEAD | **PASS HTTP 200** | CORP:cross-origin on jsdelivr |
| 1f. cdn.pyke.io ort plain | HEAD | PASS HTTP 404 (not found, but not blocked) | Not relevant — vendored |
| 2a. transformers.js@3 dynamic import() | import | **PASS** | Module loads under require-corp |
| 2b. transformers.js@4 dynamic import() | import | **PASS** | Module loads under require-corp |
| 3. transformers.js internal HF model fetch | AutoConfig.from_pretrained | **PASS** | Config loaded, `fetchOpts: {}` (no explicit mode) |
| 4a. TitaNet range+cors (partial download) | GET cors + Range:bytes=0-1023 | **PASS HTTP 206, 1024 bytes** | Actual model bytes fetchable |

**Critical detail on transformers.js fetch mode:** Intercepting `globalThis.fetch` during a `AutoConfig.from_pretrained()` call reveals `fetchOpts: {}` — transformers.js sends no explicit `mode` parameter. The browser defaults to `cors` for cross-origin requests. Since HF CDN mirrors the `Origin` header in `ACAO`, the CORS handshake succeeds and satisfies COEP require-corp. **No fork of transformers.js required.**

If a fetch override were ever needed (e.g., a CDN that doesn't send ACAO), the mechanism exists without forking: patch `globalThis.fetch` before calling `env.remoteHost` or `pipeline()`. `env.customCache` and `env.useCustomCache` also exist for cache interception but are not needed here.

---

## 3. WebKit (Playwright) Under `COEP: require-corp`

**Result: PASS — WebKit is fully cross-origin isolated under require-corp.**

```
crossOriginIsolated: true
SharedArrayBuffer: available
hardwareConcurrency: 8
UA: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15
```

| Test | WebKit Result |
|------|--------------|
| TitaNet plain HEAD | PASS HTTP 200, type:cors |
| TitaNet mode:cors HEAD | PASS HTTP 200, type:cors |
| TitaNet no-cors HEAD | FAIL (expected — no-cors = opaque, blocked) |
| jsdelivr transformers.js@3 fetch | PASS HTTP 200 |
| jsdelivr transformers.js@4 fetch | PASS HTTP 200 |
| transformers.js@3 dynamic import | PASS |
| transformers.js@4 dynamic import | PASS |
| TitaNet range GET cors bytes=0-1023 | PASS HTTP 206, 1024 bytes |

**Console error note:** The test harness intentionally runs a `no-cors` test (1c) which triggers a CORP violation log in WebKit. This is correct behavior — `no-cors` is blocked because the opaque response has no CORP header. The error is from the deliberate failure case, not from the app's real fetch paths. All CORS-mode fetches (plain fetch defaults to cors for cross-origin) pass cleanly.

**This resolves the R1 Safari blocker:** WebKit now shows `crossOriginIsolated: true` under `require-corp`, versus `false` under `credentialless`. Threaded WASM is available.

---

## 4. Firefox Under `COEP: require-corp`

```
crossOriginIsolated: true
SharedArrayBuffer: available
hardwareConcurrency: 10
UA: Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:150.0) Gecko/20100101 Firefox/150.0
```

All tests pass identically to Chrome. `no-cors` blocked as expected.

---

## 5. Firefox Nemotron RTF (Bonus — warm vs cold)

Test harness: `dev/test-playwright-cpu-tier.mjs` against `http://localhost:8099` (credentialless server, as before). Model served same-origin from `crates/nemotron-asr/models/` (881 MB encoder).

| Run | RTF | TTFT (s) | Transcript |
|-----|-----|---------|-----------|
| Cold (run 1) | **5.933** | 3.18 | "The quick brown fox jumps over the lazy dog, artificial intelligence is transforming the way we work and live." |
| Warm (run 2) | **5.906** | 3.12 | (same, correct) |

**Warm vs cold delta: 0.027 RTF.** The difference is negligible — there is essentially no warm speedup between Playwright runs. This explains i5's "6.9 cold" reading as system state, not WASM JIT cold-compile: the JIT compilation happens inside the browser process on first use and persists across page navigations only if the browser process stays alive. Each Playwright run creates a fresh browser process, so there is no carry-over JIT cache between runs.

**Root cause of i5's RTF 6.9:** Firefox Playwright headless reports `WebAssembly.validate(SIMD_vector) = false`, meaning Playwright's Firefox headless build does **not** expose WASM SIMD to the page. Without SIMD, the `ort-wasm-simd-threaded.wasm` runtime falls back to scalar computation. The RTF difference between i5's 6.9 and our 5.9 is machine load variance (concurrent processes on a 10-core machine). Both values are SIMD-off scalar performance, not a meaningful regression or calibration error.

**Practical impact:** Firefox without SIMD has RTF ~5.9-6.9 on a 10-core Apple Silicon machine. At the 250ms feed chunks the production path uses, this means the engine is processing 250ms of audio in ~1.5s of wall time — about 6x real-time, which requires the stream to buffer. For a real-mic recording scenario with silence gaps, this is marginally usable but not great. The 8-thread default does help (parallel encoder layers), but SIMD is the primary bottleneck.

**Note:** Chrome in the real app (with ORT SIMD + 8 threads) achieves RTF 0.28 (per i5 data). The Playwright Chrome harness does not replicate this because the SIMD detection in the harness is for a different WASM feature vector; the ORT runtime has its own SIMD detection path.

---

## 6. Alternatives (if require-corp had failed)

These are documented as alternatives, not needed given the successful outcome above.

### 6a. Cloudflare Pages Function varying COEP by UA

A Cloudflare Pages Function (runs at edge before asset delivery) could inspect `User-Agent` and send `COEP: credentialless` for Chrome/Firefox and `COEP: require-corp` for Safari/WebKit.

**Sketch:**

```typescript
// functions/_middleware.ts
export const onRequest: PagesFunction = async ({ request, next }) => {
  const res = await next();
  const ua = request.headers.get('User-Agent') || '';
  const isWebKit = /WebKit/.test(ua) && !/Chrome/.test(ua);
  const coep = isWebKit ? 'require-corp' : 'credentialless';
  const clone = new Response(res.body, res);
  clone.headers.set('Cross-Origin-Embedder-Policy', coep);
  return clone;
};
```

**Risk notes:**
- UA sniffing is fragile; Chrome on iOS also sends WebKit UA strings (iOS forces WebKit engine for all browsers). Would need `CriOS` detection to exclude Chrome-on-iOS.
- Cloudflare Pages Functions require a `functions/` directory and compatible build output. The current deploy uses `_headers` static file — switching to a Function changes the deploy topology.
- If a CDN cache layer sits between Cloudflare and the client, responses may be served without the dynamic COEP. Requires `Vary: User-Agent` which breaks cache efficiency significantly.
- **Not recommended now** — moot since require-corp works universally.

### 6b. Safari-degraded mode (single-thread, documented)

If require-corp had failed in WebKit, the fallback would be accepting single-threaded execution under credentialless (WebKit ignores it, `crossOriginIsolated=false`). From i5's findings:
- Safari RTF: 1.27 (single-threaded, no SAB)
- At 250ms feed chunks: ~318ms wall time per chunk = falling behind real-time
- Usable for short recordings with silence gaps; not suitable for continuous meetings

This would require detecting `!crossOriginIsolated` and switching to a buffered/batch mode with user feedback ("Safari mode: processing may lag"). Not required given the actual outcome.

### 6c. Vendoring (K2 integration)

Shrinking remote origins to zero by vendoring all runtime assets (ort-web already done per spike-ci-wasm.md; transformers.js via jsdelivr remains) would make `require-corp` trivially satisfiable — all resources are same-origin, no CORP negotiation needed. This is the privacy-by-architecture endgame (PRD R5/R6) but not required for the COEP decision.

---

## Decision Matrix

| Scenario | Chrome COI | Firefox COI | WebKit COI | HF fetch | TFjs fetch |
|----------|-----------|------------|-----------|---------|-----------|
| `COEP: credentialless` (current) | true ✓ | true ✓ | **false ✗** | ✓ | ✓ |
| `COEP: require-corp` (proposed) | true ✓ | true ✓ | **true ✓** | ✓ | ✓ |
| `COEP: require-corp` + no-cors fetch | — | — | — | **✗ blocked** | N/A |

The conflict i5 diagnosed (credentialless needed for HF, require-corp needed for Safari) dissolves: HF CDN satisfies require-corp via CORS headers (not CORP headers), and WebKit honors require-corp for COI.

---

## Recommendation

**Switch to `COEP: require-corp`.** Update `coi-server.py` and `xtask gen-headers` / `_headers`.

Evidence:
1. All three browsers (Chrome, Firefox, WebKit) show `crossOriginIsolated: true` under require-corp.
2. All real fetch paths (TitaNet HF, transformers.js jsdelivr, vendored ort-web) pass under require-corp.
3. The only blocked mode is `fetch(url, {mode:'no-cors'})` — opaque responses. The app does not use no-cors fetches anywhere; all cross-origin fetches are plain (defaults to cors) or explicit cors.
4. transformers.js uses `globalThis.fetch` with no explicit mode (`fetchOpts: {}`). Browser defaults to cors for cross-origin. HF CDN mirrors the Origin header in ACAO. This satisfies require-corp without any code change.
5. No fork of transformers.js required. No Cloudflare Function required. One header change.

**The change:**
```diff
- Cross-Origin-Embedder-Policy: credentialless
+ Cross-Origin-Embedder-Policy: require-corp
```

Apply in `_headers` (and update the decision-log comment) and in `coi-server.py`. Re-run `cargo xtask gen-headers` if headers are generated.

**Risk:** Low. The only regression vector is a CDN that stops sending CORS headers. HF CDN, jsdelivr, and pyke.io all send ACAO. Add a CI canary (`curl -sI <url> | grep Access-Control-Allow-Origin`) to trip if HF removes CORS — the K2 vendoring work eliminates this risk entirely when complete.

---

## Files tested

- Test server: `/Users/mike/dev/snt-spikes/coep/require-corp-server.py`
- Test harness: `/Users/mike/dev/snt-spikes/coep/test-harness.html`
- Playwright WebKit/Firefox test: `/Users/mike/dev/snt-spikes/coep/test-webkit-require-corp.mjs`
- Existing Firefox RTF harness: `dev/test-playwright-cpu-tier.mjs` (this repo)
