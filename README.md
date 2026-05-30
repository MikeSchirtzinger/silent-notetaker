# Silent Notetaker

**A private AI meeting notetaker that runs entirely in your browser. No backend, no accounts, no audio ever leaves your machine.**

Open a tab, hit record, and it transcribes the conversation in real time, figures out *who said what*, pulls out the decisions / action items / open questions as they happen, and even suggests the next question worth asking — all on-device, using speech and language models that run locally on your GPU and CPU.

The entire application is a **single HTML file**. There is no build step, no server requirement, and nothing to sign into. You can read every line of what it does.

---

## Why this exists

Meetings are some of the most sensitive audio you produce — strategy, hiring, finances, half-formed ideas. The mainstream AI notetakers all work the same way: they join your call, **stream your audio to someone else's servers**, and send you a summary later. That's a hard no for a lot of conversations.

The bet behind Silent Notetaker is simple: **the browser is now a capable ML runtime.** Between WebGPU (for the heavy transcription model) and WebAssembly (for everything else), a 2024+ laptop can run a real streaming speech model, a speaker-identification model, and a small language model *at the same time*, locally, with no network round-trips. So why send the audio anywhere at all?

Everything here is a demonstration of that thesis: real models, real on-device inference, zero data exfiltration.

---

## Quick start

**Requirements:** Chrome or Edge (for WebGPU), a microphone. ~2–3 GB of free disk for the model cache on first run.

```bash
git clone https://github.com/MikeSchirtzinger/silent-notetaker.git
cd silent-notetaker
./start.sh
```

`start.sh` launches a local web server with the right headers and opens the app at **http://localhost:8080**. Pick an engine from the dropdown, click **Start**, allow the mic, and talk.

> **Why a server and not just opening the file?** The on-device models use multithreaded WebAssembly, which the browser only enables when the page is "cross-origin isolated" (it needs `SharedArrayBuffer`). That requires two HTTP headers (`COOP` + `COEP`) that you can't set from a `file://` URL. `start.sh` runs a tiny server that sends them — a Rust one if you have `cargo`, otherwise a 20-line Python fallback (`coi-server.py`). A plain `python -m http.server` will *load* but run single-threaded and ~3–4× slower.

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

## The models

Everything below runs **client-side**. The transcription engine is selectable in the UI:

| Role | Model | Runs on | Size (approx) | Notes |
|------|-------|---------|---------------|-------|
| Streaming ASR (default) | **Voxtral Realtime 4B** (Mistral) | WebGPU | ~2.7 GB | Highest accuracy, true streaming |
| Fast + accurate combo | **Moonshine** + **SenseVoice** | WebGPU + WASM | ~400 MB | Moonshine for instant drafts, SenseVoice refines |
| Lightweight | **SenseVoice** (FunAudioLLM) | WASM | ~250 MB | Good accuracy, no 30s window |
| Whisper family | **Whisper large-v3-turbo / small / base** (OpenAI) | WebGPU | ~200–560 MB | Familiar baseline options |
| Speaker embeddings | **TitaNet-small** (NVIDIA NeMo) | WASM (onnxruntime-web) | ~38 MB | Bundled in this repo as `titanet.onnx` |
| Smart questions | **Qwen3-0.6B** (Alibaba) | WASM (live) + WebGPU (recap) | ~570 MB | On-device question suggestions |

All models are pulled from Hugging Face at runtime except TitaNet, which is bundled so speaker ID works immediately.

---

## Architecture

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
       │  Voxtral /       │   │  per-utterance       │   │  "ask now"       │
       │  Whisper /       │   │  192-d voice vector  │   │  teleprompter    │
       │  Moonshine+SV    │   │  → leader clustering │   │                  │
       └────────┬─────────┘   └──────────┬───────────┘   └──────────────────┘
                │ text                    │ speaker id
                ▼                         ▼
       ┌────────────────────────────────────────────┐
       │  NoteEngine (regex trigger detection)        │
       │  decisions · actions · key points · questions│
       └───────────────────┬──────────────────────────┘
                           ▼
       ┌────────────────────────────────────────────┐
       │  UI  +  IndexedDB (Dexie) — local only       │
       └────────────────────────────────────────────┘

   (optional)  ⇄  Claude Bridge (ws://localhost:8765) → Anthropic API
               for richer summaries & speaker-name inference
```

### Why it's built this way

A few decisions are worth calling out, because they're the difference between "a model runs in a browser" and "three models run at once without choking":

**One HTML file, no build.** The whole app — markup, styles, every bit of logic, even the inlined Web Workers — is in `index.html`. That makes it trivial to share, trivial to audit ("show me where the audio goes"), and impossible to hide a phone-home. The cost is a big file; the payoff is total transparency.

**WebGPU for the ASR, WASM for everything else.** The GPU is the scarce resource. The streaming speech model is by far the heaviest thing running, so it gets the GPU to itself. Speaker embedding (TitaNet) and the question LLM are deliberately run on **CPU via WebAssembly** so they can never contend with the transcription loop for GPU memory or scheduling. Different jobs, different silicon.

**Cross-origin isolation for multithreaded WASM.** With the right `COOP`/`COEP` headers the browser exposes `SharedArrayBuffer`, which unlocks multi-threaded WebAssembly — roughly 3–4× faster for the WASM models. That's why this repo ships its own header-setting server instead of telling you to use `python -m http.server`.

**Bounded memory on long meetings.** Streaming speech models have a subtle trap: inside a single `generate()` call the KV cache grows with every token, and that growth lives in GPU/native memory — *invisible* to the JS heap, so it doesn't show up in the obvious profiler. Left unchecked, an hour-long meeting balloons to multiple gigabytes and the tab freezes. The fix is to cap the per-context token budget and recycle the audio window periodically, so memory stays flat across an arbitrarily long session.

**On-device speaker ID from scratch.** There's no off-the-shelf browser pipeline for speaker diarization, so it's assembled by hand: TitaNet runs via `onnxruntime-web`, and its mel-spectrogram front-end is reimplemented in pure JavaScript and **byte-validated against the reference Python** (cosine similarity 1.000000) so the embeddings are identical. Speakers are then tracked by online "leader clustering" against a cosine threshold.

**Local-first by default.** Meetings persist in IndexedDB (via Dexie). The optional Claude bridge is the *only* path by which any data can leave the machine, it's off unless you turn it on, and it talks to a server *you* run.

---

## Optional: the Claude bridge

If you want summaries and categorization that go beyond regex triggers, `bridge.py` is a small local WebSocket server that connects the app to Claude. It's **completely optional** — the notetaker transcribes, identifies speakers, extracts notes, and suggests questions entirely on-device without it.

```bash
uv run bridge.py        # starts ws://localhost:8765
```

Auth is resolved at runtime, in this order, and **no key is ever stored in the repo**:

1. macOS Keychain (Claude Code OAuth credentials, if you use Claude Code)
2. `ANTHROPIC_API_KEY` environment variable
3. `~/.config/silent-notetaker/token`

`start.sh` launches the bridge automatically if [`uv`](https://docs.astral.sh/uv/) is installed.

---

## Project layout

| Path | What it is |
|------|------------|
| `index.html` | The entire application — UI, all logic, and the inlined transcription/audio Web Workers |
| `question-worker.js` | External Web Worker for the on-device smart-questions LLM |
| `titanet.onnx` | Bundled NVIDIA NeMo TitaNet-small speaker-embedding model (loaded at runtime) |
| `mel_fb.json` | Precomputed mel filterbank matrix for TitaNet's JS front-end |
| `server/` | Rust (axum) static server that sends the COOP/COEP isolation headers |
| `coi-server.py` | ~20-line Python fallback server with the same headers |
| `bridge.py` | Optional Claude bridge (local WebSocket → Anthropic API) |
| `start.sh` | One-command launcher: server + (optional) bridge + opens the browser |
| `Start Notetaker.command` | Double-click launcher for macOS |
| `transcription-worker.js`, `sensevoice-loader.js` | Reference copies of worker logic that `index.html` inlines |

---

## Honest limitations

- **Browser support:** needs WebGPU — Chrome or Edge today. Firefox/Safari support is still maturing.
- **First-load cost:** the first run downloads a multi-hundred-MB to ~2.7 GB model. After caching it's instant and offline.
- **Speaker diarization is the rough edge.** Online clustering can over-split (one person showing up as several speakers) or drift on long meetings, because the per-utterance segments aren't always clean single-speaker windows. Click-to-rename helps in practice, and improving this (better segmentation + global re-clustering) is the active area of work. The embeddings themselves are solid; the live clustering is where the difficulty is.
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
