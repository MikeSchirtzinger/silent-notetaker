# Architecture

This document describes how Silent Notetaker is built today, the one weakness in
its privacy story and how we close it, and the path from a single-file demo to a
modular app with a sandboxed extension marketplace — without giving up the thing
that makes it different.

The guiding principle, stated once so everything else follows from it:

> **Private by architecture, not by policy.** The cloud notetakers promise they
> won't misuse your audio. Silent Notetaker is built so the audio *physically
> cannot leave the device* — and so that anyone can verify that claim instead of
> trusting it.

Everything below is in service of making that claim **true** and **checkable**.

---

## 1. The trust boundary

The only thing that matters for the privacy claim is the **network egress
surface**: every origin the app can talk to. If that set contains no destination
your audio could flow to, the claim holds — regardless of how the code is
organized.

Here is the complete egress surface today (grep the repo to confirm):

| Destination | Why | Carries your audio? |
|---|---|---|
| `cdn.jsdelivr.net` | Transformers.js + onnxruntime-web libraries | No |
| `unpkg.com` | Dexie (IndexedDB wrapper) | No |
| `huggingface.co` (+ its model CDN hosts) | Model **weights**, downloaded once and cached | No |
| `ws://localhost:8765` | **Optional** Claude bridge — a server *you* run, off by default | Only the transcript text, only if you turn it on, and only to your own machine |

Audio is captured, turned into model inputs, and consumed **in-process**. It is
never serialized to a network call. That is the whole product.

---

## 2. Current architecture (what ships today)

The entire app is one `index.html` (~6.3k lines). It is intentionally a single
file: trivial to share, trivial to audit, impossible to hide a phone-home in. The
file is not a tangle — it's a set of clean classes that already map almost 1:1
onto future modules.

```
                          ┌─────────────────────────────┐
   🎤 Microphone  ───────▶│  AudioWorklet @ 16 kHz mono │   CaptureProcessor
                          └──────────────┬──────────────┘
                                         │ Float32 PCM → Float32RingBuffer
                 ┌───────────────────────┼────────────────────────┐
                 ▼                        ▼                        ▼
        TranscriptionManager       SpeakerEmbedder          QuestionGenerator
        (WebGPU, inlined           (TitaNet, WASM)          (Qwen3, WASM —
         worker via Blob URL)      → SpeakerTracker          external worker:
        Voxtral / Whisper /        (leader clustering)       question-worker.js)
        Moonshine+SenseVoice
                 │ text                    │ speaker id
                 ▼                         ▼
                      NoteEngine (regex trigger detection)
                 decisions · actions · key points · questions
                                   │
                                   ▼
                   App  +  IndexedDB (Dexie) — local only
                                   │
                       (optional) ⇄ ClaudeBridge → ws://localhost:8765
```

### The class map (the module seams)

| Class (in `index.html`) | Responsibility | Runtime |
|---|---|---|
| `CaptureProcessor` | AudioWorklet — mic → 16 kHz mono PCM | Audio thread |
| `Float32RingBuffer` | Lock-free-ish audio ring the ASR reads from | Main |
| `TranscriptionManager` | Drives the ASR engine; owns the streaming/recycle loop | Main + Worker (WebGPU) |
| `SenseVoiceEngine` | sherpa-onnx / SenseVoice loader (the lighter engine) | WASM |
| `SpeakerEmbedder` | TitaNet via onnxruntime-web + JS mel front-end | WASM |
| `SpeakerTracker` | Online leader clustering of voice embeddings | Main |
| `QuestionGenerator` | Smart-question LLM client (loads `question-worker.js`) | WASM Worker |
| `NoteEngine` | Decisions / actions / key points / questions extraction | Main |
| `ClaudeBridge` | WebSocket client for the optional bridge | Main |
| `App` | UI, state, orchestration, IndexedDB | Main |

Two implementation details worth knowing:

- **The transcription worker is inlined.** It lives in a
  `<script type="text/js-worker">` block and is spun up from a `Blob` URL
  (`URL.createObjectURL`). That keeps the heavy ASR worker inside the single file.
  `question-worker.js` is the one exception — a genuinely external worker.
- **`bridge.py` is embedded as display text.** The hosted app shows the bridge's
  Python source (the `ClaudeCLIBackend` / `AnthropicAPIBackend` / `MeetingContext`
  classes) so you can read and copy it. The runnable copy is the top-level
  `bridge.py`. (Keeping these in sync is a known papercut — see roadmap.)

### The honest gap

Today the privacy claim is **asserted, not enforced**. "Audio never leaves" is
true because there is no exfiltration code — but verifying that means reading 6.3k
lines and trusting that nothing was missed. There is currently **no
Content-Security-Policy**, so nothing at the platform level *stops* a network call
to an arbitrary host. That's the next section, and it's the most important change
in this document.

---

## 3. The keystone: enforce the boundary with CSP

A strict `Content-Security-Policy` (specifically `connect-src`) turns the privacy
claim from *"trust me, there's no exfil code"* into *"the browser physically
refuses to connect anywhere except these named model CDNs."* Asserted → enforced.

This is the single highest-leverage change for the HN launch, and — critically —
it is the **floor of the extension sandbox** (Section 5): third-party code cannot
phone home if the platform forbids egress.

Starting policy (hosted build — **must be validated against the live Network panel
before enforcing**, because Hugging Face redirects weight downloads to CDN hosts
that need to be in the allowlist):

```
Content-Security-Policy:
  default-src 'self';
  script-src  'self' 'unsafe-inline' blob: https://cdn.jsdelivr.net https://unpkg.com;
  worker-src  'self' blob:;
  connect-src 'self' blob: data:
              https://cdn.jsdelivr.net https://unpkg.com
              https://huggingface.co https://*.hf.co https://cdn-lfs.huggingface.co;
  img-src     'self' data: blob:;
  media-src   'self' blob:;
  style-src   'self' 'unsafe-inline';
```

Notes that matter:

- **`connect-src` is the privacy linchpin.** Even with everything else loose, a
  locked `connect-src` means audio (and anything else) has nowhere to go. This is
  the line to get right and to point auditors at.
- **The local build adds `ws://localhost:8765`** to `connect-src` for the bridge.
  The hosted build deliberately omits it (the bridge isn't available hosted), so
  the hosted egress surface is *just* library + weight CDNs.
- **`'unsafe-inline'` for scripts is a consequence of the monolith** — all JS is
  inline, so we can't use nonces/hashes yet. This is a real (secondary) weakness,
  and **modularization removes it**: once logic lives in separate module files we
  can drop `'unsafe-inline'` and move to hash/nonce-based `script-src`. So the CSP
  and the module split reinforce each other. The *egress* control (`connect-src`)
  works fully even today.
- **`blob:` and `worker-src blob:` are required** because the ASR worker is spun
  up from a Blob URL. Omitting them breaks transcription — this is exactly why the
  policy has to be browser-tested, not merged blind.

---

## 4. Target architecture: split, without a build step

The single-file architecture was the right call for a hackathon demo — one thing
to share, zero friction. It is the **wrong** call for where this is going:
user-toggleable features, lazy-loaded models, and a community extension
marketplace. None of those fit inside one fused file.

Two ideas have been conflated and should be separated:

- **Single file as an *artifact*** — "download one thing, Ctrl-F where the audio
  goes." Worth keeping as an *optional output*.
- **Single file as an *architecture*** — everything fused into one file. Worth
  dropping.

Splitting does **not** require a bundler or a build step. Native ES modules load
directly in the browser (`<script type="module">` + `import`), and dynamic
`import()` gives lazy-loading for free. Cloudflare serves 30 modules over HTTP/2 as
happily as one file. So "no build step" stays *true* after the split.

### The three layers

```
┌──────────────────────────────────────────────────────────────────────────┐
│  CORE  — lean, auditable, the trust anchor                                 │
│  audio capture · ASR · diarization · note engine · storage · CSP boundary  │
├──────────────────────────────────────────────────────────────────────────┤
│  FEATURES — first-party, lazy-loaded ES modules (downloaded when enabled)  │
│  smart-questions (Qwen) · slide/screen capture · Claude bridge · exports   │
├──────────────────────────────────────────────────────────────────────────┤
│  EXTENSIONS — third-party, sandboxed, capability-gated (the marketplace)   │
│  custom panels · summarizers · CRM/Notion/Linear push · domain templates   │
└──────────────────────────────────────────────────────────────────────────┘
```

- **CORE stays small and boring on purpose.** It's the part where "audio never
  leaves" must be provable, so it should be the part a reviewer can read in one
  sitting. It owns the CSP boundary.
- **FEATURES are lazy.** Enable smart-questions → *then* the Qwen module + weights
  download. A user who only wants transcription never pays for the rest. (The heavy
  models are already fetched on demand; this extends the same idea to the code.)
- **EXTENSIONS are untrusted by default.** See Section 5.

### Module map (extraction targets)

| Target module | Comes from |
|---|---|
| `core/audio/capture.js` | `CaptureProcessor` |
| `core/audio/ring.js` | `Float32RingBuffer` |
| `core/asr/manager.js` + `core/asr/worker.js` | `TranscriptionManager` + inlined worker |
| `core/asr/engines/{voxtral,whisper,moonshine,sensevoice}.js` | engine branches + `SenseVoiceEngine` |
| `core/diarization/embedder.js` | `SpeakerEmbedder` (+ `mel_fb.json`) |
| `core/diarization/tracker.js` | `SpeakerTracker` |
| `core/notes/engine.js` | `NoteEngine` |
| `core/store.js` | IndexedDB/Dexie code in `App` |
| `features/questions/` | `QuestionGenerator` + `question-worker.js` |
| `features/bridge/` | `ClaudeBridge` (+ `bridge.py`, no longer duplicated as display text) |
| `features/capture-screen/`, `features/export/` | the relevant `App` methods |
| `ui/` | the rest of `App` + markup/styles |

The split is **extraction, not a rewrite** — the seams already exist.

---

## 5. Extensions & the marketplace (the part that can kill the product)

**The honest warning, stated plainly:** a third-party extension marketplace is in
direct conflict with the core promise. The moment a user installs
`cool-summarizer-pro`, "audio never leaves — by architecture" degrades to "private
*unless you installed something sketchy*" — which is exactly the policy-based
promise we're beating Granola on. **If the sandbox isn't real, the marketplace
destroys the only thing that makes this special.** This section is therefore a
hard requirement, not a nice-to-have.

The sandbox rests on three controls:

1. **Network denied by default (the CSP floor).** Extensions inherit the
   `connect-src` allowlist from Section 3. An extension *cannot* open a socket or
   `fetch` to an arbitrary host — the platform refuses. Network access for an
   extension is a separate, explicit, **user-visible** grant scoped to named hosts
   (e.g. "this extension may send notes to `api.notion.com`"), shown at install and
   revocable.
2. **Isolated execution.** Extensions run in a Worker/iframe with no direct DOM or
   global access, communicating with the host over a narrow `postMessage` API —
   not by sharing the page. Raw audio and raw embeddings are **never** handed
   across that boundary; extensions receive only the data their manifest declares.
3. **Declared capabilities.** A manifest states exactly what an extension can see
   and do. The host enforces it; anything undeclared is denied.

Sketch of an extension manifest:

```jsonc
{
  "name": "notion-export",
  "version": "0.1.0",
  "capabilities": {
    "data": ["transcript.text", "notes.decisions", "notes.actions"], // NOT raw audio, NOT embeddings
    "ui": ["panel"],                                                  // may render a side panel
    "network": ["https://api.notion.com"]                            // explicit, user-approved at install
  }
}
```

Default posture: an extension sees redacted/derived data (transcript text, notes),
can render UI, and has **no** network. Everything beyond that is a visible grant.
This is what makes a *privacy-first* marketplace defensible — and it's a claim no
one else in this space can make.

---

## 6. Distribution targets

One modular source, three outputs:

| Target | What it is | Why |
|---|---|---|
| **Cloudflare web app** (primary) | The modular, lazy-loaded, marketplace-enabled app | The real product experience; modules + lazy load are free on HTTP/2 + edge cache |
| **Single-file artifact** (optional) | A tiny `build.js` inlines CORE into one `index.single.html` | Keeps the "download one auditable file / run it offline" story and the HN headline — as an *output*, not a constraint |
| **Tauri native shell** | Rust shell wrapping the web core | Unlocks the Granola-parity gaps the browser can't reach |

**Why Tauri specifically.** The browser-mic path mostly hears the room/your side
of a call. The native notetakers (Granola, Char, Meetily) win because they capture
**system audio** — the other side of the Zoom/Meet call — with no bot. A Tauri
shell (Rust + the existing web frontend) gets us:

- **System-audio capture** — ScreenCaptureKit (macOS) / WASAPI loopback (Windows)
  via a small Rust audio module, fed into the same `Float32RingBuffer`. This is the
  single biggest UX unlock and the main reason to do the Rust work.
- **Menubar / tray presence** and global hotkeys — start a meeting without finding
  a tab.
- **Calendar awareness** — "you have a meeting now, start capturing?"
- **A sellable artifact** — if/when the waitlist justifies a paid native app, it's
  already built. The free web core stays free and auditable; the native
  conveniences are the optional paid layer.

Tauri is the right shell because it reuses the web core verbatim (no second
codebase) and the native surface area is small and in Rust — which is also where
the existing `server/` (axum isolation-header server) already lives.

---

## 7. Migration roadmap

Sequenced deliberately around the Hacker News launch. **Principle: ship a clean
*working* monolith for HN, not a half-finished split.** A broken refactor at launch
is strictly worse than a clean monolith, and the big code changes below need real
browser validation (WebGPU + mic + multi-GB model downloads) that can't be faked.

| Phase | Work | Relative to HN | Needs browser test? |
|---|---|---|---|
| **0 — Harden & document** | Repo cleanup, **add + validate CSP**, this doc, README/Show HN | **Before HN** | CSP: yes |
| **1 — Modularize** | Extract ES modules per the map in §4; behavior-identical; add optional single-file `build.js` | Milestone 1 (right after) | Yes — full regression |
| **2 — Lazy + toggles** | `import()` features on demand; user-facing feature toggles; tighten `script-src` (drop `'unsafe-inline'`) | After 1 | Yes |
| **3 — Extension SDK** | Extension API, manifest, Worker/iframe sandbox, capability + network-grant enforcement; one reference extension | After 2 | Yes |
| **4 — Tauri native** | Rust shell, system-audio capture, menubar, calendar | Parallel track | Native QA |
| **5 — Marketplace** | Hosting, review/signing, install UX | After 3 + 4 | Yes |

**Validation note.** Phases 1–5 change runtime behavior and must be verified in
Chrome/Edge with a real mic and real model loads before merging. Diarization
quality (the known rough edge — online clustering over-splits) should also get a
global re-clustering pass before or alongside Phase 1, since it's the first thing a
skeptical reader will poke at.
