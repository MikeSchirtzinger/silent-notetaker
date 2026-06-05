# `apps/web/js/` — permanent JS modules (by design, not scaffolding)

This directory is the home for the JavaScript that **stays JavaScript** in the
final hybrid architecture (PRD "Proposed Rust workspace" + R2). Listing these
modules here makes "permanent by design" auditable against "temporary
scaffolding": anything in JS that is *not* one of these is migration debt to be
removed by Phase 7.

The dividing line is **policy vs. execution** (PRD R2): Rust owns the law (audio
chunking policy, engine selection, streaming loops, diarization, notes/question
scheduling, the session state machine, storage schema, the registry, extension
permissions); JS keeps the hands (DOM/UI, capture APIs, the model host
executors, the bridge socket).

## The permanent modules

| Module | Role | Status |
|---|---|---|
| `capture.js` | `getUserMedia` / `getDisplayMedia` / AudioWorklet capture and the WebAudio graph; screenshot pipeline (15 s interval, JPEG, perceptual-hash dedup). Emits `AudioChunk` samples to the engine layer via callbacks. `TranscriptionManager` in `index.html` delegates all audio capture and screenshot operations to a `CaptureGraph` instance exported from this module. | Phase 1 — implemented (task k6) |
| `transformers-host.js` | The `js-transformers` model host worker source (`EXECUTOR_WORKER_SRC`). Previously inlined in `whisper-engine.js`; exported here so any future js-transformers host can import it without duplicating it. The executor loads the transformers.js ASR pipeline and runs `transcriber(audio)` per chunk; it contains NO policy (VAD / hallucination / dedup / chunk boundaries — Rust). | Phase 1 — implemented (task k6) |
| `ort-web-loader.js` | `ort-web` runtime glue — `raiseOrtWasmThreads` (previously inlined in `nemotron-engine.js`). Raises the onnxruntime-web WASM thread count before the runtime's first session allocates the thread pool. Imported by `nemotron-engine.js`; shared by any future ort-web host. Vendored-asset wiring lands in Phase 1/2 (docs/research/spike-ci-wasm.md). | Phase 1 — implemented (task k6) |
| `bridge-client.js` | Thin WebSocket client for `bridge.py` over `ws://localhost:8765`. The `ClaudeBridge` executor class: connect / disconnect / send / sendTranscript / sendScreenshot / requestSummary / query / updateIndicator. Reconnect/backoff POLICY lives in Rust (`bridge-engine.js` → `WasmBridgeReconnect`); this module is the EXECUTOR only. Dynamically imported by `App._initBridge()` in `index.html`; the inbound message dispatch (`_handleBridgeMessage`) stays in `index.html` because it references DOM globals. | Phase 4 — pre-implemented (task k6); reconnect policy already in Rust |

## Module map

```
apps/web/js/
├── capture.js           — CaptureGraph class (audio + screenshot)
├── transformers-host.js — EXECUTOR_WORKER_SRC (ASR pipeline blob-worker source)
├── ort-web-loader.js    — raiseOrtWasmThreads (ort-web thread count trap)
├── bridge-client.js     — ClaudeBridge class (WS executor)
└── README.md            — this file
```

## Import graph

```
index.html (classic <script>)
  └── dynamic import → apps/web/js/bridge-client.js  (App._initBridge)
  └── dynamic import → apps/web/js/capture.js        (TranscriptionManager._initCapture)

nemotron-engine.js (ES module)
  └── static import  → apps/web/js/ort-web-loader.js (raiseOrtWasmThreads)

whisper-engine.js (ES module)
  └── static import  → apps/web/js/transformers-host.js (EXECUTOR_WORKER_SRC)
```

## Files NOT moved here (deliberate)

`nemotron-engine.js` and `question-worker.js` are existing, app-loaded modules
that stay at the repo root. Moving them is an E1 step, not a C1/k6 one, because
the app loads them by root-relative path:

- `index.html` does `import('./nemotron-engine.js')` and
  `new Worker('question-worker.js', { type: 'module' })`.
- `nemotron-engine.js` resolves the wasm package relative to its own URL.

Relocating either requires editing those load sites in `index.html`, which is
out of scope for this task. E1 (integration) performs these moves together with
the `silent-web` typed-boundary migration (Phase 3, row 22 / Task G3), where
`question-worker.js`'s protocol becomes typed commands from `silent-web` and
`nemotron-engine.js`'s glue migrates onto the typed event boundary.
