# Silent Notetaker

**A private AI meeting notetaker that runs entirely in your browser. No backend, no accounts, no audio ever leaves your machine.**

Open a tab, hit record, and it transcribes the conversation in real time, figures out *who said what*, pulls out the decisions / action items / open questions as they happen, and even suggests the next question worth asking — all on-device, using speech and language models that run locally on your GPU and CPU.

The application policy — speaker diarization, note extraction, smart-question scheduling, storage, the engine state machine — is written in **Rust, compiled to WebAssembly**, with a thin HTML/JS shell for the UI. That means there is now a build step, but it buys you something stronger than "read the single file": the build is **reproducible**, and the running app shows you the SHA-256 of its own wasm so you can verify that the binary you are running is the one this source produces. See [Verify the binary you are running](#verify-the-binary-you-are-running).

---

## Private *by architecture*, not by policy

Every mainstream AI notetaker works the same way: it joins your call, **streams your audio to someone else's servers**, runs the AI there, and sends a summary back. The good ones are careful about it — encryption, SOC 2, "we delete the audio after transcription," contractual no-training clauses. Even the privacy-marketed ones (Granola, for instance, reportedly valued around $1.5B) still send your meeting to cloud LLMs and store it on their infrastructure. Their privacy is a **promise**, backed by a company.

Silent Notetaker makes the promise structurally unnecessary. The audio is captured, fed to the models, and consumed **in-process**. It is never serialized into a network request. And you don't have to take that on faith — here is the *complete* list of origins the app ever contacts:

| Destination | Why | Receives your audio? |
|---|---|---|
| `cdn.jsdelivr.net` | Transformers.js runtime library | No |
| `unpkg.com` | Dexie IndexedDB library | No |
| `cdn.pyke.io` | onnxruntime-web runtime loader | No |
| `huggingface.co`, `*.hf.co`, `cdn-lfs.huggingface.co`, `cdn-lfs-us-1.huggingface.co` | Model **weights**, downloaded once and cached in your browser | No |
| `ws://localhost:8765` | *Optional* Claude bridge — a server **you** run, off by default | Only transcript text, only if you enable it, only to your own machine |

That's it. There is no analytics endpoint, no telemetry host, no crash-reporting
origin. The egress list above is generated from `registry/models.toml` by
`cargo xtask gen-headers` and enforced as a `Content-Security-Policy` `connect-src`
directive — the browser refuses connections to anything not on this list. Open your
browser's network panel and watch.

(See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for how this is generated and enforced, and [`SECURITY.md`](SECURITY.md) for what counts as a privacy-boundary vulnerability.)

There are whole categories of conversation — legal, medical, hiring, finance, M&A, journalism with sources — where "the audio left the building" is simply a non-starter. This is built for those.

---

## Verify the binary you are running

The privacy story is only as good as the binary that actually runs in your tab. The application logic is Rust compiled to WebAssembly, so "read the single HTML file" no longer covers it. Instead, the build is **reproducible** and the app **shows you the hash of its own wasm** — so you can confirm the hosted app is built from this source, byte for byte, without trusting us.

**Three commands — clone, build, compare:**

```bash
git clone https://github.com/MikeSchirtzinger/silent-notetaker.git
cd silent-notetaker
./scripts/build-wasm.sh
```

`build-wasm.sh` prints the SHA-256 of each deployed wasm module, e.g.:

```
3de64f5b8237f32b223193281763ad60a1190edeb9f3113ced73048ff4215dae  crates/silent-web/pkg/silent_web_bg.wasm
4e9e12f6d8b0fe2c649b325b7f4396d199f85cbef3f88e192af009c27d2b80b5  crates/nemotron-asr/pkg/nemotron_asr_bg.wasm
```

Now open the running app, go to **Settings → About & integrity**, and you'll see the same two hashes with a green **VERIFIED** badge — the app computed them live (`crypto.subtle`, in your browser) over the wasm it actually loaded and matched them against the `wasm-hashes.txt` manifest shipped with the deploy. **The hash from your local build equals the hash the app reports.** That is the whole proof: hosted == source.

**What makes it reproducible (and what can break it):** the output bytes are pinned by `rust-toolchain.toml` (Rust 1.95.0), the committed `Cargo.lock`, the release profile in `Cargo.toml`, and **wasm-pack 0.13.1** (which bundles wasm-bindgen 0.2.100 + wasm-opt v117). The script remaps machine-specific source paths (your `CARGO_HOME`, the toolchain sysroot, the checkout path) to stable tokens via `--remap-path-prefix`, so the hash does **not** depend on where you cloned or where your toolchain lives — a fresh clone in any directory on any machine with the same toolchain produces the same bytes. A *different* wasm-pack version is the most common reason a hash won't match; the script prints the version it used. CI rebuilds the wasm twice from clean and fails if the two builds differ, then publishes the `wasm-hashes` manifest as a build artifact.

---

## Quick start

**Requirements:** Chrome or Edge (for WebGPU), a microphone. ~2–3 GB of free disk for the model cache on first run.

```bash
git clone https://github.com/MikeSchirtzinger/silent-notetaker.git
cd silent-notetaker
./start.sh
```

`start.sh` launches a local web server with the right headers and opens the app at **http://localhost:8080**. Pick an engine from the dropdown, click **Start**, allow the mic, and talk.

> **Why a server and not just opening the file?** The on-device models use multithreaded WebAssembly, which the browser only enables when the page is "cross-origin isolated" (it needs `SharedArrayBuffer`). That requires two HTTP headers (`COOP` + `COEP`) that you can't set from a `file://` URL. `start.sh` runs a tiny server that sends them — a Rust one if you have `cargo`, otherwise a small, dependency-free Python fallback (`coi-server.py`). A plain `python -m http.server` will *load* but run single-threaded and ~3–4× slower.

First load downloads the selected model from Hugging Face and caches it in the browser. After that it works offline.

---

## What it does

- 🎙️ **Live transcription** — streaming speech-to-text as you talk, with multiple model options trading off speed vs. accuracy.
- 🧑‍🤝‍🧑 **Speaker identification** — labels each line by speaker (`S1`, `S2`, …) using on-device voice embeddings. Click any speaker tag to rename them; the name propagates to every line.
- 🗂️ **Automatic note extraction** — categorizes the conversation in real time into **Decisions**, **Action Items**, **Key Points**, and **Open Questions** using trigger detection.
- 💡 **Smart questions** — a small on-device LLM suggests a good question to ask *right now* (a clarifying question, a devil's-advocate challenge, or a follow-up), plus a fuller question recap when you stop.
- 🖼️ **Slide / screen capture** — optionally grab tab audio + screenshots of shared slides.
- 💾 **Local-first storage** — every meeting is saved to IndexedDB in your browser. Nothing is uploaded.
- 📋 **Clean export** — copy the whole meeting as structured Markdown.
- 🤖 **Optional Claude bridge** — if you want sharper summaries, you can connect it to Claude (see below). Entirely optional; the app is fully functional without it.

---

## The hard parts (why this is more than "a model runs in a browser")

The interesting engineering is in making **three models run at once, on different silicon, without choking** — and in keeping a streaming model alive for an hour without the tab freezing. Three highlights, all documented in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md):

- **The invisible WebGPU runaway.** Inside one `generate()` call, a streaming model's KV cache grows with every token — and that memory lives in **GPU/native space, invisible to the JS heap profiler**. At ~0.52 MB/token the old 4096-token cap ballooned to ~2 GB and froze the tab around the 5-minute mark. The fix is two caps (a 320-token budget and a 45-second audio window) feeding one recycle loop that re-anchors a fresh context at "now" — flat memory across an arbitrarily long meeting, no audio dropped at the seam.
- **GPU for speech, WASM for everything else.** The GPU is the scarce resource, so the heaviest model (Voxtral) gets it alone. Speaker-ID (TitaNet) and the question LLM (Qwen) run on WASM/CPU so they can **never contend** with the streaming loop. Different jobs, different silicon.
- **Speaker ID from scratch.** No browser library does diarization, so it's hand-built: TitaNet via onnxruntime-web, with its mel-spectrogram front-end reimplemented in pure JS and **byte-validated against the reference Python — cosine similarity 1.000000**. TitaNet-small was picked in a [6-model bake-off](eval/) (it beat the app's old 20 hand-features, which were near-random — EER 36.7%, 6 speakers shattered into 17) — though that 0% EER is on *clean* speech (LibriSpeech test-clean); live clustering on messy meeting audio is the real weak link. Speakers are tracked by online "leader clustering" on a tuned cosine threshold.

---

## The models

Everything below runs **client-side**. The transcription engine is selectable in the UI:

| Role | Model | Runs on | Size (approx) | Notes |
|------|-------|---------|---------------|-------|
| Streaming ASR (default) | **Voxtral Realtime 4B** (Mistral) | WebGPU | ~2.7 GB | Best accuracy of these options, true streaming |
| Fast + accurate combo | **Moonshine** + **SenseVoice** | WebGPU + WASM | ~400 MB | Moonshine for instant drafts, SenseVoice refines |
| Lightweight | **SenseVoice** (FunAudioLLM) | WASM | ~250 MB | Good accuracy, no 30s window |
| Whisper family | **Whisper large-v3-turbo / small / base** (OpenAI) | WebGPU | ~200–560 MB | Familiar baseline options |
| Speaker embeddings | **TitaNet-small** (NVIDIA NeMo) | WASM (onnxruntime-web) | ~38 MB | Bundled in this repo as `titanet.onnx` |
| Smart questions | **Qwen3-0.6B** (Alibaba) | WASM (live) + WebGPU (recap) | ~570 MB | On-device question suggestions |

All models are pulled from Hugging Face at runtime except TitaNet, which is bundled so speaker ID works immediately.

---

## Architecture

The UI shell — markup, styles, the engine loaders, the inlined transcription worker — lives in `index.html` and a handful of small ES modules (`diarization-engine.js`, `notes-engine.js`, …). The application *policy* lives in Rust crates under `crates/` and compiles to two wasm modules (`silent-web`, `nemotron-asr`). This is no longer a "read the single file" story, so we replaced it with a stronger one: the build is reproducible and the app verifies its own wasm hash in-app (see [Verify the binary you are running](#verify-the-binary-you-are-running)). Internally the Rust core is a clean set of crates (`silent-core` contracts, `silent-diarization`, `silent-notes`, `silent-storage`, `silent-inference`, `nemotron-asr`, …) driving a thin JS host.

```
                          ┌─────────────────────────────┐
   🎤 Microphone  ───────▶│  AudioWorklet @ 16 kHz mono │
                          └──────────────┬──────────────┘
                                         │ Float32 PCM
                 ┌───────────────────────┼────────────────────────┐
                 ▼                        ▼                        ▼
       ┌──────────────────┐   ┌──────────────────────┐   ┌──────────────────┐
       │  ASR engine      │   │  Speaker embedder    │   │  Smart-questions │
       │  (WebGPU)        │   │  TitaNet (WASM)      │   │  Qwen3 (WASM)    │
       └────────┬─────────┘   └──────────┬───────────┘   └──────────────────┘
                │ text                    │ speaker id
                ▼                         ▼
       ┌────────────────────────────────────────────┐
       │  NoteEngine (regex trigger detection)        │
       └───────────────────┬──────────────────────────┘
                           ▼
       ┌────────────────────────────────────────────┐
       │  UI  +  IndexedDB (Dexie) — local only       │
       └────────────────────────────────────────────┘

   (optional)  ⇄  Claude Bridge (ws://localhost:8765) → Claude
```

**Full design notes, the network trust boundary, and the roadmap are in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** — including how the single-file core becomes a modular, lazy-loaded app with a sandboxed extension system, and a Tauri native shell for system-audio capture.

---

## Free core, open extensions

The notetaker — capture, transcription, speaker ID, note extraction, smart questions — is **free and will stay free**, and the core stays auditable. The plan from here is to open up an **extension layer** so the community can build on top of it (custom panels, summarizers, exports to Notion/Linear/CRM, domain-specific templates).

The one rule that makes a *privacy-first* extension ecosystem possible: extensions are **sandboxed and network-denied by default**. An extension sees only the data it declares, runs isolated from the page, and gets no network access unless you grant it explicitly. That's how the marketplace can grow without ever undermining the "your audio never leaves" guarantee. Design details are in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#5-extensions--the-marketplace-the-part-that-can-kill-the-product).

*Built by [Brevity Ventures](https://brevity.ventures) — we build private, on-device AI.*

---

## Optional: the Claude bridge

If you want summaries and categorization that go beyond regex triggers, `bridge.py` is a small local WebSocket server that connects the app to Claude. It's **completely optional** — the notetaker transcribes, identifies speakers, extracts notes, and suggests questions entirely on-device without it.

```bash
uv run bridge.py        # starts ws://localhost:8765
```

**Connecting to Claude takes zero setup if you use [Claude Code](https://claude.com/claude-code).** The bridge drives the `claude` CLI in headless mode, so it reuses the subscription you're already logged into — no API key, no token to paste. Auth is resolved at runtime, in this order, and **no key is ever stored in the repo**:

1. **The `claude` CLI (your Claude subscription).** If `claude` is on your `PATH` and logged in, the bridge uses it — nothing else to configure. It runs `claude -p` with `ANTHROPIC_API_KEY` scrubbed from the child env, so it authenticates with your subscription, not a pay-per-use key.
2. **`ANTHROPIC_API_KEY`** (or a saved `~/.config/silent-notetaker/token`). Used only if the CLI isn't available. This **bills your API account**, not your subscription.

> Why not just hand the keychain OAuth token to the API? Anthropic's API rejects subscription OAuth tokens used outside Claude Code, so driving the real CLI is the supported, low-friction path.

`start.sh` launches the bridge automatically if [`uv`](https://docs.astral.sh/uv/) is installed.

---

## Project layout

| Path | What it is |
|------|------------|
| `index.html` | The entire application — UI, all logic, and the inlined transcription/audio Web Worker |
| `question-worker.js` | External Web Worker for the on-device smart-questions LLM |
| `titanet.onnx` | Bundled NVIDIA NeMo TitaNet-small speaker-embedding model (loaded at runtime) |
| `mel_fb.json` | Precomputed mel filterbank matrix for TitaNet's JS front-end |
| `server/` | Rust (axum) static server that sends the COOP/COEP isolation headers |
| `coi-server.py` | Small, dependency-free Python fallback server with the same headers |
| `bridge.py` | Optional Claude bridge (local WebSocket → `claude` CLI / your subscription) |
| `start.sh` | One-command launcher: server + (optional) bridge + opens the browser |
| `Start Notetaker.command` | Double-click launcher for macOS |
| `overview.html` | A scrollytelling "six build decisions" walkthrough of the engineering |
| `docs/ARCHITECTURE.md` | Full architecture, the network trust boundary, and the roadmap |
| `dev/` | Development scratch / test harnesses (not part of the app) |

---

## Honest limitations

- **Browser support:** needs WebGPU — Chrome or Edge today. Firefox/Safari support is still maturing.
- **First-load cost:** the first run downloads a multi-hundred-MB to ~2.7 GB model. After caching it's instant and offline.
- **Speaker diarization is the rough edge.** Online clustering can over-split (one person showing up as several speakers) or drift on long meetings, because the per-utterance segments aren't always clean single-speaker windows. Click-to-rename helps in practice, and improving this (better segmentation + global re-clustering) is the active area of work. The embeddings themselves are solid; the live clustering is where the difficulty is.
- **Browser mic, not system audio (yet).** Today it captures your microphone, so it hears the room and your side of a remote call well, but not the far side of a Zoom/Meet stream cleanly. Native **system-audio capture** is the main item on the roadmap (see the Tauri shell in `docs/ARCHITECTURE.md`).
- **Hardware:** running a 4B speech model in a browser tab wants a reasonably modern GPU. The lighter engines (SenseVoice, Whisper base/small) are there for weaker machines.

---

## Tech stack & credits

- **[Transformers.js](https://huggingface.co/docs/transformers.js)** — in-browser model inference (WebGPU)
- **[onnxruntime-web](https://onnxruntime.ai/docs/get-started/with-javascript/web.html)** — WASM inference for the speaker model
- **[Dexie.js](https://dexie.org/)** — IndexedDB wrapper for local storage
- **[axum](https://github.com/tokio-rs/axum)** — the Rust isolation-header server
- Models: **Voxtral** (Mistral AI), **Moonshine** (Useful Sensors), **SenseVoice** (FunAudioLLM), **Whisper** (OpenAI), **TitaNet** (NVIDIA NeMo), **Qwen3** (Alibaba)

## License

Code is [MIT](./LICENSE). Bundled and downloaded models retain their own upstream licenses (see the note in `LICENSE`).
