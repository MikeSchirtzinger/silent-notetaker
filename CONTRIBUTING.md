# Contributing to Silent Notetaker

Silent Notetaker is a single-file, on-device AI meeting notetaker built on the
principle that **audio never leaves the device — by architecture, not by policy**.
The core is free, stays free, and stays auditable. The place where the community
builds is the extension layer.

This document covers how to run the app locally, how the repo is laid out, the
conventions in the codebase, and how to propose and land changes.

---

## Quick start

**Requirements:** Chrome or Edge (WebGPU is required — Firefox and Safari support
is not yet stable enough), a microphone, ~2–3 GB free disk for the model cache on
first run.

```bash
git clone https://github.com/MikeSchirtzinger/silent-notetaker.git
cd silent-notetaker
./start.sh
```

`start.sh` launches a local web server and opens **http://localhost:8080**. Pick
an engine from the dropdown, click **Start**, allow the microphone, and talk.

### Why you need a server (not `file://`)

The on-device models use multithreaded WebAssembly. The browser only enables
`SharedArrayBuffer` — which multithreaded WASM requires — when the page is
"cross-origin isolated," meaning it was served with two HTTP headers: `COOP:
same-origin` and `COEP: require-corp`. You cannot set response headers from a
`file://` URL.

`start.sh` runs one of two tiny servers that send these headers:

- **Rust server** (`server/` — axum): used if you have `cargo` on your PATH. It
  also serves the report-only Content Security Policy header used for egress
  auditing. Build it with `cd server && cargo build --release`.
- **Python fallback** (`coi-server.py`): a small fallback with no dependencies
  beyond the standard library. `python -m py_compile coi-server.py` must pass
  before commit.

A plain `python -m http.server` will load the app but run models single-threaded,
roughly 3–4 times slower.

### Optional: the Claude bridge

`start.sh` also starts `bridge.py` automatically if
[`uv`](https://docs.astral.sh/uv/) is installed. The bridge is a local WebSocket
server that connects the app to Claude for richer summaries. It is entirely
optional — the core app transcribes, diarizes, extracts notes, and suggests
questions without it.

```bash
uv run bridge.py    # starts ws://localhost:8765
```

---

## Repo layout

| Path | What it is |
|---|---|
| `index.html` | **The entire application** — UI, all logic, and the inlined transcription Web Worker. This is the canonical source. |
| `question-worker.js` | External Web Worker for the on-device smart-questions LLM (the one file that is a genuine external worker today) |
| `titanet.onnx` | Bundled NVIDIA NeMo TitaNet-small speaker-embedding model |
| `mel_fb.json` | Precomputed mel filterbank matrix used by TitaNet's JS front-end |
| `bridge.py` | Optional Claude bridge (local WebSocket → `claude` CLI / Anthropic API) |
| `coi-server.py` | Small Python fallback server with COOP/COEP isolation headers |
| `server/` | Rust (axum) server — same isolation headers, also serves the CSP Report-Only header |
| `start.sh` | One-command launcher: picks a server, (optionally) starts the bridge, opens the browser |
| `Start Notetaker.command` | Double-click launcher for macOS |
| `docs/ARCHITECTURE.md` | Full architecture, the network trust boundary, and the migration roadmap |
| `docs/EXTENSIONS.md` | Design spec for the extension system (Phase 3 — not yet implemented) |
| `docs/SHOW_HN.md` | Show HN draft |
| `dev/` | Development scratch and test harnesses — not part of the app |

---

## The single-source-of-truth rule

**Today: the canonical app code lives entirely in `index.html`.**

The application is intentionally a single file. Every class — `CaptureProcessor`,
`Float32RingBuffer`, `TranscriptionManager`, `SpeakerEmbedder`, `SpeakerTracker`,
`QuestionGenerator`, `NoteEngine`, `ClaudeBridge`, `App` — is defined inline.
The transcription Web Worker is also inlined, living in a `<script
type="text/js-worker">` block spun up via a Blob URL at runtime.

`question-worker.js` is the one genuine external worker today, because the
question LLM runs in a separate process and benefits from an external file for
debugging.

**What this means for contributors:**

- If you are fixing a bug in transcription, speaker ID, note extraction, or the
  UI — the change goes in `index.html`.
- Do not duplicate logic between `index.html` and any external JS file. If
  something needs to move out, move it out entirely and remove it from `index.html`.
- The `bridge.py` source is shown as display text inside the app *and* lives as a
  runnable top-level file. Keeping these in sync is a known papercut; if you edit
  `bridge.py` you must update the display copy inside `index.html` as well.

### The modular future (roadmap, not today)

`docs/ARCHITECTURE.md` §4 describes how the app will eventually split into native
ES modules without a build step. That split is Phase 1 of the migration roadmap,
scheduled after the HN launch. Until then, **do not begin the module split in
`index.html`** — a half-finished refactor at launch is strictly worse than a clean
monolith. Discuss any structural change in an issue first.

---

## Conventions

### Code style

- The codebase is vanilla JavaScript (no TypeScript, no framework). Keep it that
  way. The app has no build step on purpose.
- Prefer `class` definitions with clear method names over loose functions. The
  existing class map (`TranscriptionManager`, `SpeakerEmbedder`, etc.) is the
  logical structure; new features should extend or add to it, not spread logic
  across free-floating event handlers.
- Comments should explain *why*, not *what*. The code is public and will be read
  by skeptical engineers checking the privacy claim; make their job easy.
- Do not leave `console.log` debug statements in committed code. The existing logs
  are intentional status/telemetry for the user's browser console; new logs should
  follow the same pattern.

### Privacy conventions (non-negotiable)

- **No new egress.** Do not add any `fetch`, `XMLHttpRequest`, WebSocket, or other
  network call that sends user data to a host not already in the CSP allowlist
  (`cdn.jsdelivr.net`, `unpkg.com`, `huggingface.co`, and its model CDN hosts).
  Any new remote dependency requires explicit discussion and a CSP update.
- **No audio escaping the boundary.** Raw PCM, encoded audio, mel spectrograms,
  and voice embeddings must never appear in a network payload, a postMessage to an
  extension, or any storage that could be read by third-party code.
- The optional Claude bridge (`ws://localhost:8765`) sends transcript text only,
  never audio. If you extend the bridge protocol, preserve that invariant and
  document it.

### Servers

- Changes to `server/src/main.rs` must pass `cd server && cargo check` before
  commit.
- Changes to `coi-server.py` must pass `python -m py_compile coi-server.py`
  before commit.
- The Content Security Policy header is currently **Report-Only** in both servers.
  Do not promote it to enforcing without first browser-testing against the live
  Network panel (Hugging Face redirects model downloads to CDN subdomains that may
  not all be in the allowlist). See `docs/ARCHITECTURE.md` §3.

---

## How to propose a change

1. **Open an issue first** for anything beyond a typo or obvious bug. Describe the
   problem, not just the solution.
2. **Fork the repo**, create a branch from `main`.
3. **Make the change** following the conventions above. Test in Chrome or Edge with
   WebGPU enabled and a real microphone.
4. **Describe what you tested** in the PR description. "Ran locally" is not enough
   — note which engine you used, whether speaker ID was active, and whether the
   change touches anything in the trust boundary.
5. **Do not add a build step.** The repo has no bundler and should not acquire one.
   Native ES modules, Blob Workers, and dynamic `import()` are the tools.

### What the core stays

The core notetaker — audio capture, transcription, speaker ID, note extraction,
smart questions, local storage — is **free and will remain free**. The source will
remain auditable: no obfuscation, no minification of the shipped file, no feature
flags that hide behavior from the reader.

### Where the community builds

Extensions are where the community adds value on top of the core: custom panels,
domain-specific note templates, exports to Notion/Linear/CRM, integrations with
calendar or ticketing systems. The extension system is designed so that this growth
**never undermines the privacy guarantee** — extensions are sandboxed, network-denied
by default, and see only the data they declare.

The extension API is designed but not yet implemented (Phase 3 of the roadmap).
See [`docs/EXTENSIONS.md`](docs/EXTENSIONS.md) for the full spec. If you want to
build an extension now, open an issue to discuss — the API design can be influenced
by real use cases before the implementation is locked.

---

## Getting help

- **Architecture questions:** read `docs/ARCHITECTURE.md` first — it covers the
  trust boundary, the CSP approach, the class map, and the migration roadmap in
  depth.
- **Extension design questions:** read `docs/EXTENSIONS.md`.
- **Bugs or unexpected behavior:** open an issue with your browser version, the
  engine you selected, and the browser console output.
