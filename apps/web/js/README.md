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

| Module | Role | Lands in |
|---|---|---|
| `capture.js` | `getUserMedia` / `getDisplayMedia` / AudioWorklet capture and the WebAudio graph; emits typed `AudioChunk` events to Rust. | Phase 1 (rows 4, 5, 26) |
| `transformers-host.js` | The `js-transformers` model host worker. Receives typed commands from Rust (load/feed/generate/recycle) and returns events; **no policy** (proven in `docs/research/spike-jshost.md`). | Phase 5 (rows 7, 10, 11, 19) |
| `ort-web-loader.js` | `ort-web` runtime glue (onnxruntime-web loader + vendored-asset wiring). | Phase 1/2 |
| `bridge-client.js` | Thin WebSocket client for `bridge.py` over `ws://localhost:8765`; reconnect/backoff **policy** lives in Rust. | Phase 4 (rows 27, 28) |

The files in this directory are **placeholders** at the end of Task C1: they
carry the contract docstring for each module but no logic, so the workspace
layout is established without changing app behavior. Each is filled in by its
owning phase via the strangler-fig pattern `nemotron-engine.js` already proved.

## Files NOT moved here yet (deliberate — see Task C1 / E1)

`nemotron-engine.js` and `question-worker.js` are existing, app-loaded modules
that **stay at the repo root for now**. Moving them is an E1 step, not a C1 one,
because the app loads them by root-relative path:

- `index.html` does `import('./nemotron-engine.js')` and
  `new Worker('question-worker.js', { type: 'module' })`.
- `nemotron-engine.js` resolves the wasm package relative to its own URL.

Relocating either requires editing those load sites in `index.html`, which is
out of scope for the contracts task (C1 may make at most one path-line edit, and
that budget was spent re-homing the moved `nemotron-asr/pkg/` path inside
`nemotron-engine.js`). E1 (integration) performs these moves together with the
`silent-web` typed-boundary migration (Phase 3, row 22 / Task G3), where
`question-worker.js`'s protocol becomes typed commands from `silent-web` and
`nemotron-engine.js`'s glue migrates onto the typed event boundary.
