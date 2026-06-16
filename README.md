# Silent Notetaker

**Meeting notes that never leave your machine. No backend, no account, no upload.**

A private AI meeting notetaker that runs entirely in your browser. Open a tab, hit record, and it transcribes in real time, figures out *who said what*, and pulls out decisions, action items, key points, and open questions as they happen — all on-device, using speech and language models that run locally on your CPU and GPU. Nothing is ever sent to a server.

> Top-5 Global Demo at AI Tinkerers.

---

## Why

Every mainstream AI notetaker works the same way: it joins your call, **streams your audio to someone else's servers**, runs the AI there, and sends a summary back. The good ones are careful — encryption, SOC 2, "we delete the audio after transcription." Even the privacy-marketed ones (Granola, for example) still send your meeting to cloud LLMs and store it on their infrastructure. Their privacy is a **promise**, backed by a company.

Silent Notetaker makes the promise structurally unnecessary. The audio is captured, fed to the models, and consumed **in-process** — it is never serialized into a network request. This is privacy **by architecture, not by policy**, and you can verify it: open your browser's network panel and watch. The only things the app ever fetches are JavaScript runtimes and model weights (downloaded once, then cached). A `Content-Security-Policy` `connect-src` allowlist — enforced, not report-only — makes the browser itself refuse any connection outside that set.

There are whole categories of conversation — legal, medical, hiring, finance, M&A, journalism with sources — where "the audio left the building" is a non-starter. This is built for those.

---

## Features

- **Live transcription** — streaming speech-to-text as you talk.
- **Speaker identification** — labels each line by speaker (`S1`, `S2`, …) using on-device voice embeddings. Click any tag to rename a speaker; the name propagates everywhere.
- **Automatic note extraction** — sorts the conversation in real time into **Decisions**, **Action Items**, **Key Points**, and **Open Questions**.
- **Live meeting outline & smart questions** — a small on-device LLM builds an outline as you go and suggests a sharp question worth asking *right now*.
- **Slide / screen capture** — optionally grab tab audio and screenshots of shared slides.
- **Local-first storage** — every meeting is saved on your device. Nothing is uploaded.
- **Clean export** — copy the whole meeting as structured Markdown.
- **Optional Claude bridge** — connect to Claude for sharper summaries via a local server *you* run. Entirely optional; the app is fully functional without it.

---

## How it works

The whole pipeline runs client-side, in your browser:

```
  🎤 Microphone
       │
       ▼
  AudioWorklet @ 16 kHz mono
       │  Float32 PCM
       ├───────────────────────┬───────────────────────────┐
       ▼                       ▼                            ▼
  Nemotron ASR          TitaNet diarization        Qwen3 notes / outline
  (CPU / WASM,          (CPU / WASM,               (CPU / WASM live,
   streaming)            speaker embeddings)        WebGPU recap)
       │ text                  │ speaker id                 │
       └───────────┬───────────┘                           │
                   ▼                                        ▼
        Note extraction (Rust)  ───────────────▶  UI + local storage
                                                  (Rust/WASM, on-device)

  (optional)  ⇄  Claude bridge (ws://localhost:8765 — a server you run)
```

The design splits work across silicon so nothing contends: the **default ASR (Nemotron streaming)** runs on **CPU/WASM**, which leaves the **GPU free for on-device Qwen** to generate notes and questions. Heavier engines like Voxtral take the GPU when selected. The application *policy* — engine selection, diarization, note triggers, question scheduling, storage, the recording state machine — is written in **Rust, compiled to WebAssembly**, driving a thin HTML/JS shell.

---

## Quickstart

**Requirements:** Chrome or Edge (most complete WebGPU support), a microphone, and a few hundred MB of free disk for the first-run model cache.

```bash
git clone https://github.com/MikeSchirtzinger/silent-notetaker.git
cd silent-notetaker
./start.sh
```

`start.sh` launches a local server with the right headers and opens the app at **http://localhost:8080**. Pick an engine, click **Start**, allow the mic, and talk. The first run downloads the selected model from Hugging Face and caches it in your browser — after that it works offline.

> **Why a server and not just opening the file?** The on-device models use multithreaded WebAssembly, which the browser only enables when the page is "cross-origin isolated" (it needs `SharedArrayBuffer`). That requires two HTTP headers (`COOP` + `COEP`) you can't set from a `file://` URL. `start.sh` runs a tiny server that sends them — a Rust (axum) one if you have `cargo`, otherwise a dependency-free Python fallback (`coi-server.py`). A plain `python -m http.server` will *load* the app but run single-threaded and several times slower.

**Building the WebAssembly modules** (only needed if you change the Rust crates):

```bash
./scripts/build-wasm.sh
```

The app is fully static and deploys to **Cloudflare Pages** (the `_headers` file carries the isolation + CSP headers). There is no backend to deploy.

---

## Architecture

The UI shell — markup, styles, engine loaders, the inlined transcription worker — lives in `index.html` and a handful of small ES modules (`nemotron-engine.js`, `diarization-engine.js`, `storage-engine.js`, …). The application *policy* lives in Rust crates under `crates/`, compiled to WebAssembly:

| Crate | Responsibility |
|---|---|
| `silent-core` | Shared contracts, session state machine, exporters |
| `nemotron-asr` | Cache-aware RNN-T streaming ASR (native + wasm32) |
| `silent-diarization` | TitaNet speaker embeddings + speaker tracking |
| `silent-notes` | Note trigger extraction + Qwen smart-question scheduling |
| `silent-storage` | On-device storage schema + migrations |
| `silent-inference` | JS-host engine command dispatch |
| `silent-audio` | Audio chunking policy |
| `silent-extension-sdk` | Sandboxed extension permission model |
| `silent-web` | The wasm-bindgen boundary that exposes all of the above to the UI |

Each ES-module engine loader is a thin wrapper that delegates every policy decision to its corresponding wasm surface. Full design notes, the complete network trust boundary, and the roadmap are in **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)**.

---

## Models

Everything runs client-side. The transcription engine is selectable in the UI.

| Role | Model | Runs on |
|---|---|---|
| Streaming ASR (default) | **Nemotron streaming 0.6B** (NVIDIA) | CPU / WASM |
| Streaming ASR (premium) | **Voxtral Realtime 4B** (Mistral) | WebGPU |
| Lightweight ASR | **SenseVoice** (FunAudioLLM), **Moonshine** (Useful Sensors) | WASM / WebGPU |
| ASR (familiar baseline) | **Whisper** large-v3-turbo / small / base (OpenAI) | WebGPU |
| Speaker embeddings | **TitaNet-small** (NVIDIA NeMo) | WASM (onnxruntime-web) |
| Notes & smart questions | **Qwen3** 0.6B / 1.7B (Alibaba) | WASM (live) + WebGPU (recap) |

Models are pulled from Hugging Face at runtime and cached locally.

---

## Tech stack

`Rust` · `WebAssembly` · `WebGPU` · `onnxruntime-web` · `Transformers.js` · `axum` · vanilla JS (no framework, no bundler) · Cloudflare Pages

---

## Privacy statement

Audio is captured, turned into model inputs, and consumed in-process. It is **never** written to a network request. There is no analytics endpoint, no telemetry host, and no crash-reporting origin in the codebase. The complete list of origins the app can contact is the CSP `connect-src` allowlist (Hugging Face for weights, two CDNs for runtimes, and the optional localhost Claude bridge), generated from `registry/models.toml` and enforced by the browser. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full trust boundary.

---

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The core notetaker — capture, transcription, speaker ID, note extraction, smart questions, local storage — is **free and stays free**, and the source stays auditable.

---

## License & credits

Code is [MIT](./LICENSE). Bundled and downloaded models retain their own upstream licenses. Models by NVIDIA, Mistral AI, OpenAI, Alibaba, FunAudioLLM, and Useful Sensors.

*Built by [Brevity Ventures](https://brevity.ventures) — we build private, on-device AI.*
