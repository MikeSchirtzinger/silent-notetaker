# PRD: Hybrid Rust Core Refactor (Rust/WASM logic + swappable model hosts)

## Status

v2 draft for engineering review, 2026-06-04.

v1 framed this as a pure Rust/WASM/WebGPU rewrite. v2 supersedes that framing
after a four-branch review (`main`, `hn-prep`, `nemotron-rust`, `qwen-notes`)
and explicit product decisions. The goal is unchanged: a verifiable, private,
open source notetaker with a clean Rust architecture, swappable model backends,
and an extension system that does not weaken the privacy promise. The path is
different: hybrid by design, migrated strangler-fig inside the shipping app,
with zero feature loss.

## Locked product decisions

These are decided, not open questions. Everything else in this document serves
them.

1. **Hybrid, not pure Rust.** Rust owns all application logic and policy.
   Model execution happens in swappable *hosts*: `rust+ort-web` and
   `js+transformers.js`. A JS host is a first-class production backend, not
   migration scaffolding. Reimplementing transformers.js model classes in Rust
   is explicitly out of scope.
2. **Zero feature loss.** Every feature in the shipping UI is preserved.
   Appendix A is the parity contract; a phase that loses a feature from
   Appendix A does not ship.
3. **Voxtral stays.** It is the highest-accuracy engine and the headline
   capability ("a 4B realtime model in a browser tab"). It runs in the
   `js+transformers` host because that is the only runtime that can run it.
   That is a consequence of decision 1, not a compromise.
4. **Swappable models are the product.** Users choose their point on the
   accuracy ↔ hardware curve: Voxtral for maximum accuracy on strong WebGPU
   machines, Nemotron for streaming ASR on modest CPUs and non-Chromium
   browsers, Whisper/Moonshine tiers in between. Adding a model must be cheap.
5. **The notes model is optional and extensible.** Qwen is the default, not a
   requirement. "No notes model" is a supported configuration (transcript-only
   mode), and the notes slot must accept future small models without core
   changes.
6. **The UI does not change.** Migration is strangler-fig under the existing
   `index.html` UI, the pattern `nemotron-engine.js` already proved. There is
   never an "old app" and a "new app."

## Product thesis

Silent Notetaker should become the reference open source browser-native AI
notetaker: no backend account, no uploaded audio, no hidden hosted inference,
and no model weights committed to the repo. The hosted Cloudflare app is the
easy path for users; the local clone remains auditable and reproducible.

The current JavaScript implementation proves the product is possible, and the
`nemotron-rust` branch proves the substrate: a standalone Rust crate
(`nemotron-asr`) already runs streaming ASR in the browser through WASM with
validated accuracy and latency. The refactor generalizes that proof. The hard
parts are systems problems — audio buffering, streaming policy loops,
long-running memory correctness, typed boundaries, deterministic validation —
and those belong in strictly typed, memory-safe Rust.

Code quality is also a business artifact. This repo and its published crates
are Brevity's public engineering credential. A crate other engineers depend on
is a permanent, compounding advertisement; a codebase reviewers can audit in an
afternoon is the privacy claim made legible.

## Goals

- Move all application logic and policy into a Rust workspace whose browser
  target is `wasm32-unknown-unknown`: strictly typed, memory safe,
  deterministically testable without a browser.
- Execute models through swappable hosts (`rust+ort-web`, `js+transformers`),
  selected per engine by registry data. WebGPU is consumed through the
  runtimes' execution providers, not through a bespoke `wgpu` layer.
- Make model choice a user-facing feature across three slots: ASR (required,
  multiple tiers), speaker embeddings, and notes/questions (optional).
- Keep all inference local to the user's browser or local native shell. No
  hosted inference is allowed in the free web app.
- Fetch model artifacts from Hugging Face at runtime. The repo stores model
  registry metadata, links, revisions, licenses, and hashes, not weights.
- Preserve the privacy claim as an enforceable boundary: audio, embeddings,
  mel tensors, and model activations never leave the device. No telemetry,
  ever.
- Support Cloudflare Pages as the primary hosted distribution target.
- Publish `nemotron-asr` (and later other crates) to crates.io as flagship
  evidence of engineering quality.
- Raise Rust quality using the ADA Rust engineer bar: understand root causes,
  preserve intentional placeholders, document any warning allowances, and
  validate before marking work complete.

## Non-goals

- No backend transcription or hosted LLM fallback.
- No mock model results in product code. A mock can exist only in tests named
  as a mock; it does not satisfy feature acceptance.
- No committed model weights, including ONNX, GGUF, safetensors, or external
  data blobs.
- No JS application logic in the final architecture. JS remains only where it
  is the right tool: DOM/UI, browser capture APIs
  (`getUserMedia`/`getDisplayMedia`/AudioWorklet), and model host executors.
- No Rust purity test. "Port Voxtral's runtime to Rust" is out of scope; it
  buys no user value and carries enormous regression risk.
- No feature descoping. The refactor never ships a phase that loses a feature
  from Appendix A.
- No extension marketplace until the sandbox and network-deny model are
  enforced.
- No claim of model parity until a real browser run proves it.
- No telemetry or analytics egress — not now, not later. This is an invariant,
  not a current state.

## Current state

Four branches matter. The refactor starts from their union, not from
`hn-prep` alone.

**`hn-prep` (shipping app):**

- `index.html`: 6,283 lines — UI, orchestration, audio capture, transcription,
  diarization, note extraction, smart questions, history, exports, bridge
  client, diagnostics, and worker bootstrap in one file. Appendix A inventories
  its full feature surface.
- `question-worker.js`: Qwen smart-question worker (108 lines).
- `bridge.py`: local Claude bridge over `ws://localhost:8765` (557 lines),
  also embedded in the app for download.
- `server/`: small Rust Axum local server for cross-origin isolation headers.
- `_headers`: Cloudflare Pages COOP/COEP plus report-only CSP.
- `eval/`: speaker-embedder bake-off harness (Python + JS), TitaNet
  byte-validation, `bench_results.json`.
- `titanet.onnx` + `mel_fb.json`: bundled weights — conflicts with the
  weight-free requirement and is fetched from an unpinned
  `FluffyBunnies/titanet-small-onnx/resolve/main` URL.
- `docs/ARCHITECTURE.md`, `docs/DIARIZATION.md`, `docs/EXTENSIONS.md`,
  `docs/SHOW_HN.md`.

**`nemotron-rust` (the proven Rust foundation):**

- `nemotron-asr/`: a standalone Rust crate implementing cache-aware RNN-T
  streaming ASR for NVIDIA `nemotron-speech-streaming-en-0.6b`:
  - Pure-Rust mel frontend (`audio.rs`), wasm-safe SentencePiece detokenizer
    (`vocab.rs`), encoder-cache carry-forward streaming loop (`streaming.rs`).
  - `AsrBackend` split: native `ort` (pinned `=2.0.0-rc.12`) and browser
    `ort-web 0.2.1` (bridges to onnxruntime-web 1.24). The wasm binary is
    ~424 KB.
  - Golden test (`tests/golden.rs`) and a Python reference harness
    (`reference/`), including `reference/FINDINGS.md`.
  - **Measured**: RTF 0.139× native, ~0.28–0.5× in-browser WASM, TTFT ~0.65 s
    after the edge-guard fix, 100% word accuracy on the golden clip.
    Browser-validated 2026-06-04.
  - **Measured**: CPU beat WebGPU for this model class (small INT8 encoder,
    irregular ops) — see `reference/FINDINGS.md`. This finding shapes the
    runtime strategy below.
- `nemotron-engine.js`: the integration pattern this whole refactor
  generalizes — a Rust/WASM engine swapped under the unchanged UI. Artifacts:
  `encoder.onnx` (INT8, ~881 MB), `decoder_joint_fp32.onnx` (~36 MB),
  `tokenizer.model` (~251 KB), fetched from a configurable
  `__NEMOTRON_MODEL_BASE` (currently unpinned — a registry candidate).
- Speaker merge-by-rename, PerfMonitor, and Qwen model options
  (`Qwen3-0.6B-ONNX` / `Qwen3-1.7B-ONNX`) with device-tier auto-defaults.

**`qwen-notes`**: precursor line for the Qwen notes/device-tier work.

**`main`**: pre-HN-prep baseline.

The current architecture already identifies the right product invariants:
private by architecture, local inference, Hugging Face for weights not user
data, cross-origin isolation for threaded WASM, CSP `connect-src` as the
egress enforcement surface, extensions sandboxed and network-denied by
default. Those carry forward unchanged.

**Step zero of this refactor is merging `nemotron-rust` into `hn-prep`**
(after the already-planned instrumented real-mic validation run), so every
phase below starts from the union of proven work.

## Target users

- Privacy-sensitive professionals who cannot send meeting audio to cloud
  notetakers.
- Users on modest hardware: no WebGPU, 8 GB RAM, Firefox or Safari. The CPU
  engine tier (Nemotron, small Whisper, Moonshine) exists for them; they are a
  target market, not a degraded fallback.
- Engineers and researchers who want an auditable local AI notetaker.
- Rust engineers who discover Brevity through `nemotron-asr` on crates.io.
- Open source contributors who want to swap models, add local-first workflows,
  or build extensions without touching the core audio pipeline.
- Mike as the first power user and reviewer, with Claude reviewing the plan
  and implementation quality.

## Product requirements

### R1. Browser-hosted local inference, tiered by hardware

The Cloudflare-hosted app must perform all inference on the client device. The
full engine lineup runs in Chrome/Edge with WebGPU. The CPU engine tier must
run in any browser that supports threaded WASM under COOP/COEP — including
Firefox and Safari. This is a capability the JS-only app never had; it is an
acceptance row, not a footnote.

Acceptance:

- Opening the Cloudflare app loads code and model artifacts only from allowed
  origins.
- Starting a meeting downloads the selected model from Hugging Face into
  browser cache and transcribes locally.
- Browser network inspection shows no audio, embedding, mel tensor,
  transcript, or meeting-note payload sent to third-party hosts.
- `crossOriginIsolated === true` on the hosted and local app.
- If WebGPU is unavailable, engines that require it are shown as unavailable
  with the reason, and the UI recommends a CPU-tier engine. No silent
  fallback, and never hosted inference.
- Nemotron (and other CPU-tier engines) transcribe real microphone audio in
  Firefox and Safari.

### R2. Rust owns policy; JS executes

The production app's behavior must live in Rust crates. The dividing line is
policy versus execution:

**Rust owns (the law):** audio chunking policy; engine selection and device
tiers; streaming policy loops, including Voxtral's token/audio two-cap context
recycle; diarization (SpeakerTracker, stop-time global recluster,
rename/merge-by-rename); note trigger extraction and open-question tracking;
smart-question scheduling and type rotation; word-correction application; the
recording-session state machine; storage schema and migrations; the model
registry; retry/backoff and memory budgets; extension permissions; export
formatting and timestamp modes.

**JS keeps (the hands):** DOM and the existing UI; `getUserMedia`,
`getDisplayMedia`, AudioWorklet capture and the WebAudio graph; the
transformers.js host worker; ort-web loader glue; the bridge WebSocket client;
clipboard. Each permanent JS module is listed in the workspace layout so
"permanent by design" is auditable against "temporary scaffolding."

Acceptance:

- A JS host adapter contains no policy: no thresholds, no retry logic, no
  chunk-size or recycle decisions. Those arrive as typed commands and config
  from Rust. Reviewers can verify this by reading the adapter.
- Every policy has deterministic Rust tests that do not require a browser.
  Voxtral's two-cap recycle — the hardest-won bug fix in the app — becomes a
  unit-tested Rust policy module instead of loop code in a JS closure.
- Browser-only functionality is tested with `wasm-bindgen-test` and a real
  browser runner.

### R3. Swappable model slots as a user-facing feature

Every subsystem that can reasonably vary sits behind an explicit Rust trait or
message contract — and for models, the swap is exposed to users.

**Model slots:**

| Slot | Active | Options | Off allowed? |
|---|---|---|---|
| ASR | exactly one | Voxtral Realtime 4B (max accuracy, WebGPU), Nemotron 0.6B streaming (CPU, all browsers), Whisper large-turbo/small/base/tiny, Moonshine, SenseVoice, Dual (Moonshine drafts + SenseVoice refiner) | No |
| Speaker embeddings | one | TitaNet-small (CPU by design — avoids GPU contention with Voxtral) | Labels can be hidden; pipeline may be disabled |
| Notes + questions | zero or one | Qwen3-0.6B (default), Qwen3-1.7B (high-tier default), future small models | **Yes — transcript-only mode is fully supported** |

SenseVoice stays in the lineup: its artifacts move from the k2-fsa Space path
to a first-party Hugging Face repo (as was done for TitaNet), pinned and
hashed like every other model (see Appendix C, decision log). It is also
load-bearing — Dual mode's refiner is SenseVoice.

**Selection policy lives in Rust as registry data, not code:** device-tier
detection (WebGPU availability, memory, `crossOriginIsolated`, thread count)
maps to per-tier defaults — the mechanism `nemotron-rust` already implements
for Qwen, generalized. The UI surfaces recommendations; user choice always
wins and persists.

Required swap points (architecture-level, carried from v1): `AudioSource`,
`Resampler`, `VadEngine`, `AsrEngine`, `SpeakerEmbeddingEngine`,
`SpeakerTracker`, `QuestionGenerator`, `NoteExtractor`, `NotesEngine`,
`ModelFetcher`, `ModelHost`, `Storage`, `Exporter`, `ExtensionHost`.

Acceptance:

- Adding a model within an existing family (another Whisper size, another
  Qwen size) is a registry entry — zero code.
- Adding a new model family means implementing the engine trait; it does not
  touch UI state management or audio capture.
- Disabling the notes model yields a clean transcript-only experience: no dead
  panels, no error toasts, no degraded copy.
- Engine switching is well-defined: selection applies at the next recording
  start. Changing models mid-recording is accepted and queued with a friendly
  notice — "takes effect for your next meeting," or a refresh prompt where a
  reload is required — never a silent failure and never a hard rejection.
- Extension APIs cannot request raw audio or embeddings because those types
  are not exposed in the extension capability vocabulary.

### R4. Hugging Face model registry

The repo must contain a typed model registry, not model weights. The registry
is the single source of truth: engine selection, CSP generation, egress
manifest, license display, and cache verification all derive from it.

Registry fields:

- `id`: stable local id, for example `asr.nemotron.streaming_0_6b`.
- `task`: `asr`, `speaker_embedding`, `notes`, `vad`.
- `provider`: `huggingface`.
- `repo` and `revision`: exact commit SHA or immutable revision. `main` is
  not acceptable for production defaults (the current TitaNet URL violates
  this today).
- `files`: required artifacts, each with path, size, sha256, and purpose.
  Multi-file artifact sets are first-class — Voxtral is ~2.7 GB across many
  files; Nemotron is three files totaling ~917 MB — with per-file download
  progress events.
- `host`: `rust-ort-web`, `js-transformers`, or `js-sherpa` (the sherpa-onnx
  Emscripten runtime SenseVoice uses). The hybrid is honest and auditable
  because the registry records it.
- `execution_provider`: `cpu` or `webgpu` (per host availability).
- `precision`: available dtype variants (fp32, fp16, q8, q4f16, ...).
- `device_tiers`: per-tier defaults and requirements (minimum memory, WebGPU
  needed, browser support), encoding the Qwen tier-default mechanism as data.
- `memory_budget_mb`: expected peak memory budget.
- `cache`: `cache-api` or `transformers-idb`, plus hash-verification policy —
  verify once per revision, record the verification, do not re-hash multi-GB
  files on every load.
- `license` and `license_verified`: upstream license, and a flag set only
  after a human has read it (NVIDIA's Nemotron and TitaNet terms, Voxtral,
  Qwen — Phase 0 exit criterion).
- `network_origins`: derived allowlist entries needed for fetch.
- `validation`: golden fixture ids and expected outputs.

Initial registry entries:

| Role | Model | Host | Required action |
|---|---|---|---|
| Streaming ASR (flagship Rust) | NVIDIA `nemotron-speech-streaming-en-0.6b` (encoder INT8 ~881 MB + decoder ~36 MB + tokenizer) | rust-ort-web | Pin the artifact source (currently configurable/unpinned), record hashes, verify NVIDIA license. |
| Streaming ASR (max accuracy) | `onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX` (~2.7 GB multi-file) | js-transformers | Pin revision, hash all files, encode WebGPU+memory tier requirement. |
| Whisper ASR | `onnx-community/whisper-large-v3-turbo`, `whisper-small.en`, `whisper-base.en`, `whisper-tiny.en` | js-transformers | Pin revisions; keep as mid-tier engines. |
| Fast ASR | `onnx-community/moonshine-base-ONNX` | js-transformers | Pin revision; validate draft latency. |
| Speaker embeddings | `FluffyBunnies/titanet-small-onnx` | rust-ort-web (after Phase 2) | Remove repo weight, pin revision + hash, preserve mel validation. |
| Notes + questions | `onnx-community/Qwen3-0.6B-ONNX`, `onnx-community/Qwen3-1.7B-ONNX` | js-transformers | Pin revisions; encode device-tier defaults as registry data. |
| SenseVoice ASR | currently the k2-fsa Space path (sherpa-onnx wasm harness + model, ~253 MB) | js-sherpa | Re-host artifacts to a first-party HF repo (as with TitaNet), pin revision + hashes. |

Acceptance:

- CI fails if model weight files are committed outside explicitly allowed tiny
  fixtures.
- Every production model resolves from the registry and verifies by hash.
- A stale or moved model link fails loudly with a model-resolution error, not
  a broken meeting UI.
- Model licenses are displayed or linked in the app and documented for open
  source users; `license_verified` is true for every shipped default.

### R5. Privacy boundary

The privacy boundary is a product feature and a testable contract.

Requirements:

- Raw PCM, compressed audio, mel features, embeddings, logits, hidden states,
  and model activations never cross a network boundary.
- Transcript text may only leave the browser through explicit user action:
  copying/exporting, a user-approved extension, or the local Claude bridge.
- The Claude bridge egress target is `ws://localhost:8765` — the user's own
  machine. Hosted builds keep it in CSP `connect-src` (correcting v1, which
  would have silently dropped the bridge feature from hosted deployments).
  Localhost is inside the user's trust boundary; third-party origins are not.
- The bridge is backend-agnostic by design: Claude (CLI or API) today; a
  Codex backend and other local agent CLIs are a planned future option,
  explicitly out of scope for this refactor.
- Third-party extensions are network-denied by default.
- The app maintains a machine-readable egress manifest generated from the
  model registry and extension grants.
- **No telemetry or analytics egress, ever.** Companies add metrics later;
  this one does not. The egress manifest makes the promise checkable.

Acceptance:

- Automated browser tests intercept `fetch`, WebSocket, EventSource, and
  worker fetch paths and fail on unexpected origins.
- CSP `connect-src` is enforced (not report-only) before third-party
  extensions ship.
- The Network panel validation checklist is part of release gating.

### R6. Cloudflare distribution and generated egress policy

Cloudflare Pages is the primary public host.

Requirements:

- Static deploy artifact contains app shell, WASM bundles, JS glue, CSS,
  `_headers`, model registry metadata, and integrity metadata.
- Model artifacts are not deployed to Cloudflare by default.
- COOP/COEP keeps multithreaded WASM enabled. **COEP is `require-corp`** (switched
  from `credentialless` on 2026-06-05; see decision log). `require-corp` is required
  for WebKit/Safari cross-origin isolation, and HF CDN satisfies it via CORS headers
  (`docs/research/spike-coep.md`). The invariant: cross-origin fetches must stay
  CORS-eligible (no `no-cors` mode).
- **CSP is generated, not audited.** `xtask` generates `_headers` and the
  local-server CSP from the registry plus extension grants; CI checks
  freshness. A hand-edited CSP that drifts from the registry fails the build.
- Current third-party runtime origins (jsdelivr/unpkg for transformers.js,
  `cdn.pyke.io` for ort-web's onnxruntime-web loader) are an evaluation
  target: vendoring those runtimes into the deploy bundle shrinks
  `connect-src` to `self` + Hugging Face + `ws://localhost:8765`. That is a
  real tightening of the privacy boundary. Constraint: Cloudflare Pages'
  25 MB/file limit — the threaded/JSEP onnxruntime wasm is close to it.
- Service worker caching only if it preserves the egress boundary and has
  stale-metadata tests.

Acceptance:

- Fresh-profile Cloudflare run downloads a selected model and transcribes real
  microphone audio.
- Returning-profile run starts from browser cache without refetching unchanged
  model files (and without re-hashing multi-GB files — see R4 cache policy).
- The deploy gate is an `xtask` command, not a shell-script convention:
  deploy fails if weights are present in the bundle, if any file exceeds
  25 MB, or if `_headers` is stale relative to the registry.
  (`deploy-cloudflare.sh` currently has none of these checks.)

### R7. Extension system

The extension system must be capability-based and network-denied by default.

Requirements:

- Extensions run in isolated workers or sandboxed iframes.
- Extensions receive only data declared by manifest and approved by the user.
- Raw audio, embeddings, mel tensors, and model activations are not grantable
  capabilities.
- Network grants are origin-scoped, visible at install, revocable, and applied
  only to that extension context.
- Extension API contracts are versioned and tested.

Acceptance:

- A reference extension can read transcript text and notes, render a panel,
  and export Markdown without raw audio access.
- An extension attempting undeclared network access is blocked by CSP and
  reported in logs.
- An extension requesting raw audio is rejected at manifest validation.

### R8. Reproducible, verifiable builds

`wasm-pack` ends the no-build-step era — `docs/ARCHITECTURE.md`'s single-file
auditability story must be retired honestly and replaced with something
stronger: verify the binary you are running.

Requirements:

- Pinned toolchain via `rust-toolchain.toml`; deterministic wasm builds.
- CI publishes the sha256 of every deployed wasm artifact.
- The app displays the hash of its own running wasm (settings/about panel) so
  anyone can verify hosted == source.
- The README documents the three-command reproduction path from fresh clone to
  matching hash.

### R9. Performance budgets as regression gates

The measured baselines exist; encode them as gates, not aspirations. Budgets
live in the registry per engine and run in the golden harness as CI/release
gates.

| Gate | Budget | Measured baseline |
|---|---|---|
| Nemotron TTFT (browser) | ≤ 1.0 s | ~0.65 s |
| Nemotron RTF (browser WASM) | ≤ 0.5× | ~0.28–0.5× |
| Nemotron RTF (native) | ≤ 0.2× | 0.139× |
| Voxtral 10-minute session | flat memory (two-cap recycle), no DEVICE-LOST | current shipped behavior |
| All engines, 10-minute run | no unbounded heap growth | crash-diag ring buffer evidence |

PerfMonitor and the crash-diagnostics ring buffer are formalized on `tracing`
with a wasm subscriber; the localStorage ring buffer and `dumpDiag()` recovery
trail are preserved as features (Appendix A).

### R10. Public crates: the customer-credibility mechanism

Publishing is a goal, not a side effect. `nemotron-asr` is already shaped for
it (standalone by design, MIT OR Apache-2.0, keywords, description).

Requirements:

- `nemotron-asr` on crates.io with docs.rs API docs and examples, a README
  with the benchmark table (RTF/TTFT/accuracy, native and browser), semver
  discipline enforced by `cargo-semver-checks`, an MSRV policy, and a
  CHANGELOG.
- Later candidates once stabilized: the parameterized mel frontend
  (`silent-audio`) and the TitaNet embedder.
- Crate READMEs link back to Silent Notetaker and Brevity. The crate is the
  ad; the app is the proof.

## Proposed Rust workspace

```text
apps/
  web/                         # the existing UI (index.html evolves in place)
    js/                        # permanent JS modules (by design, not scaffolding):
      capture.js               #   mic/tab/AudioWorklet capture
      transformers-host.js     #   js+transformers model host worker
      ort-web-loader.js        #   ort-web runtime glue
      bridge-client.js         #   thin WebSocket client for bridge.py
  cloudflare/                  # deploy bundle config; _headers GENERATED here
crates/
  silent-core/                 # domain contracts: commands, events, errors,
                               #   registry types; no browser deps
  silent-audio/                # ring buffers, resampling, chunking core,
                               #   parameterized mel frontends (see Validation)
  nemotron-asr/                # existing crate, moves in as-is; publishes to
                               #   crates.io (keeps standalone-buildable shape)
  silent-inference/            # engine traits, host adapters, selection policy
  silent-diarization/          # TitaNet embedder, SpeakerTracker, stop-time
                               #   recluster, rename/merge policy
  silent-notes/                # trigger extraction, open questions, notes &
                               #   question policy, correction application
  silent-storage/              # IndexedDB via indexed_db_futures; Dexie v2
                               #   migration
  silent-extension-sdk/        # manifests, capabilities, host messages
  silent-web/                  # wasm-bindgen boundary; generates TypeScript
                               #   types (ts-rs) for the UI
  silent-server/               # local Axum dev server (exists today as server/)
xtask/                         # build, model audit, CSP/_headers generation,
                               #   deploy gates, golden harness
bridge.py                      # local Claude bridge (Python, unchanged)
```

`silent-core` must not depend on browser APIs. Browser integration belongs in
`silent-web`. This keeps the core testable without a GPU and prevents browser
glue from leaking into product logic.

Subsystems that v1 left unhomed, now homed: the Claude bridge stays
`bridge.py` + `bridge-client.js` (permanent, local-only); tab audio and
screenshots stay in `capture.js` feeding typed events to Rust; history/search
and word corrections are Rust (`silent-storage`, `silent-notes`); the
smart-questions teleprompter is Rust policy (`silent-notes`) rendered by the
existing UI.

## Core contracts

The exact names can change during design, but the architecture must preserve
these boundaries — and the contract must be implementable in a browser.

**The v1 synchronous trait was unimplementable as designed**: ort-web is
async-only, and you cannot block the browser main thread
(`nemotron-asr/src/backend_web.rs` documents exactly this). The contract is
therefore async-first and event-shaped, with the full lifecycle
`nemotron-engine.js` discovered empirically: load (with per-file progress),
warm-up, feed, finalize, reset, stats.

```rust
#[non_exhaustive]
pub enum EngineEvent {
    LoadProgress { file: String, loaded: u64, total: u64 },
    Ready,
    Draft { text: String, range: TimeRange },     // e.g. Moonshine dual mode
    Partial { text: String, range: TimeRange },   // streaming, will be revised
    Final { text: String, range: TimeRange },
    Stats(EngineStats),                           // TTFT, RTF, chunk-ms
}

pub trait AsrEngine {
    fn id(&self) -> ModelId;
    fn capabilities(&self) -> AsrCapabilities;
    async fn load(&mut self, events: &mut dyn EventSink) -> Result<(), AsrError>;
    async fn warm_up(&mut self) -> Result<(), AsrError>;
    async fn feed(&mut self, chunk: AudioChunk) -> Result<Vec<EngineEvent>, AsrError>;
    async fn finalize(&mut self) -> Result<Vec<EngineEvent>, AsrError>;
    fn reset(&mut self);
    fn stats(&self) -> EngineStats;
}
```

Contract rules:

- **Shared `AsrError` enum**, not per-engine associated error types. Engine
  swapping needs uniform error handling.
- **Dispatch strategy is named, not improvised**: async-fn-in-trait is not
  dyn-safe, so engine selection uses enum dispatch
  (`enum AnyAsrEngine { Nemotron(..), JsHost(..), .. }`). Implementers do not
  invent five different strategies.
- `NotesEngine` and `QuestionGenerator` follow the same shape
  (`load`/`generate` with typed kinds: live question, stop-time recap, final
  notes). `NoteExtractor` (trigger regexes) is pure Rust policy with no model.
  The orchestrator handles `None` in the notes slot as a first-class state.
- `SpeakerEmbeddingEngine` and `SpeakerTracker` carry from v1, plus
  `recluster()` (stop-time global recluster per `docs/DIARIZATION.md`) and
  rename/merge events that must survive reclustering.
- `ModelFetcher` is async with per-file progress and hash verification
  (verify-once-per-revision policy from R4).

**The JS host adapter** (`JsHostEngine`) implements these same traits by
driving a transformers.js worker over the versioned command protocol. Policy —
chunk sizes, the two-cap recycle, when to feed, when to recycle — is decided
in Rust and arrives at the worker as commands; the worker executes
generate/decode steps and returns events. This is how Voxtral, Whisper,
Moonshine, and Qwen run under Rust law.

Error types are domain-specific in library crates. `anyhow` is acceptable only
at binary or `xtask` boundaries.

## The UI boundary

Since the UI does not change, the Rust↔UI contract carries everything — it is
the most important API in the system and is specified, versioned, and typed:

- Commands (UI → core) and events (core → UI) are versioned Rust types;
  event enums are `#[non_exhaustive]`.
- TypeScript definitions are generated from the Rust types (ts-rs or tsify);
  the unchanged UI is typed against the core, and a boundary change that would
  break the UI fails at build time, not at runtime.
- **Command-log replay**: the boundary supports capturing a session's command
  stream and replaying it deterministically. For a long-running streaming
  audio app this is the difference between "cannot reproduce" and a failing
  test.

## Model runtime strategy

This is decided, not a spike. The evidence already exists.

**Three hosts, selected per engine by registry data:**

- `rust-ort-web`: Nemotron (CPU/WASM — `reference/FINDINGS.md` measured CPU
  beating WebGPU for this model class) and TitaNet (CPU by design, avoiding
  GPU contention with Voxtral — a feature of the architecture, not a
  limitation).
- `js-transformers`: Voxtral (its model class, streaming mel generator, and
  KV-cache machinery exist only inside transformers.js), Whisper family,
  Moonshine, Qwen.
- `js-sherpa`: SenseVoice, on its existing sherpa-onnx Emscripten harness with
  the artifacts re-hosted first-party and pinned. It runs as it does today;
  moving it off the main thread is an optional later improvement, not a
  parity requirement.

**WebGPU arrives through the runtimes** — transformers.js `device: 'webgpu'`
and ort-web's WebGPU EP where it wins. The v1 raw-`wgpu` spike is cut: no
model in the lineup consumes it. `wgpu` remains a future note for custom
kernels only.

What remains genuinely unproven is enumerated as Phase 0 spikes below — with
the two spikes v1 asked for that are *already done* marked as done, with
evidence.

## Rust engineering bar

The implementation must follow the ADA Rust engineer rules.

Baseline workflow:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Workspace lints (starting point — tune with documented rationale):

```toml
[workspace.lints.rust]
unsafe_code = "forbid"        # relax per-crate only with documented invariants
missing_docs = "warn"         # becomes deny on crates published to crates.io

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"          # production paths; tests may allow
expect_used = "warn"
```

Toolchain policy: edition 2024 for new crates (`nemotron-asr` migrates from
2021 when it moves in); MSRV recorded in `rust-toolchain.toml` and treated as
latest-stable-minus-two; MSRV bumps are semver-minor for published crates.

Supply chain (credibility features, not chores):

- `cargo-deny` (licenses, advisories, bans) and `cargo-audit` in CI.
- `cargo-semver-checks` on published crates.
- Known risk pins, tracked with upgrade checkpoints: `ort =2.0.0-rc.12` is a
  release-candidate pin (plan an upgrade checkpoint at Phase 5); `ort-web
  0.2.x` is young with a small maintainer pool (pin + vendored runtime assets
  + upstream contributions).

Quality rules (carried from v1, unchanged): understand before fixing; treat
repeated warnings as design signals; fix root causes; preserve labeled
placeholders; every `#[allow]` carries a rationale; no `unwrap()`/`expect()`
in production paths unless the invariant is local, documented, and
unrecoverable; typed errors and contracts on public APIs; clear ownership of
shared state; streaming loops have cancellation, backpressure, and
memory-budget tests; comments explain why, not what. Cleanup PRs report
warning counts before/after with categorized rationale for what remains.

One named refactor deliverable: the native and wasm chunking loops in
`nemotron-asr` (`streaming.rs` vs `backend_web.rs`) duplicate the same
chunking + greedy RNN-T decode logic and risk silent divergence. They
consolidate behind one chunking core (Phase 1).

## Validation plan

Validation must distinguish true functionality from placeholders.

### CI gates

- `cargo fmt --all --check`, `cargo check`, `cargo test`,
  `cargo clippy -- -D warnings` (workspace, all targets).
- `wasm-pack test --headless --chrome`. Note: ort-web fetches its
  onnxruntime-web runtime at load; CI must vendor those assets (or explicitly
  allow that origin) or browser-wasm tests will flake.
- `cargo-deny`, `cargo-audit`, `cargo-semver-checks` (published crates).
- `xtask model audit`: no weights in repo; all registry artifacts have pinned
  revisions, hashes, sizes, licenses, and `license_verified`.
- `xtask egress audit`: generated CSP/`_headers` is fresh relative to registry
  + extension grants.
- `xtask deploy gate`: weight-free bundle, 25 MB/file limit, headers fresh.
- Perf gates from R9 against golden fixtures.
- Link check for docs and registry links.

### Golden tests

Keep the existing harnesses (`eval/`, `nemotron-asr/reference/`,
`tests/golden.rs`) and standardize the convention: every engine ships
`goldens/<engine>/` with fixture, harness, expected outputs, and reproduction
instructions.

**Two mel frontends, not one.** They are different on nearly every axis and
must never be "unified":

| | TitaNet frontend | Nemotron frontend |
|---|---|---|
| Mel bands | 80, slaney | 128, slaney |
| Window | periodic Hann | symmetric Hann |
| Spectrum | magnitude path w/ per-feature normalization | power spectrum, log-guard 2⁻²⁴, no normalization |
| Validation | byte-validated, cosine 1.000000 vs reference | golden transcript, 100% word accuracy |

`silent-audio` treats the frontend as parameterized config with per-model
golden fixtures (port `eval/js/validate.mjs` into Rust tests). A PR that
"deduplicates" the two recipes into one is a correctness bug.

Additional goldens: resampling tolerance fixtures; speaker-embedding cosine
thresholds; diarization on clean and intentionally messy meeting clips;
ASR golden ranges (not brittle exact strings unless deterministic); note
extraction across decisions/actions/key points/open questions, ported
verbatim from the current trigger behavior before any improvement.

### Browser release tests

These cannot be faked and are required before marking a phase complete:

- Fresh Chrome or Edge profile, Cloudflare hosted URL,
  `crossOriginIsolated === true`, real microphone permission.
- At least one real model downloaded from Hugging Face.
- A 10-minute recording with steady memory behavior.
- Network panel shows no unexpected egress.
- Stop-time export produces transcript and notes.
- Browser refresh preserves cached meeting state.
- **CPU-tier engine (Nemotron) transcribes in Firefox and Safari.**
- **Appendix A parity checklist passes for every feature the phase touched.**

Any phase that cannot run these tests is labeled `NEEDS-BROWSER-TEST`, not
complete.

## Migration phases

Strangler-fig inside the shipping app. Every phase ships into the deployed
product behind the unchanged UI; each ends with the browser release tests and
the Appendix A parity check green. Pure-logic ports come first (low risk,
immediate test value); the hot streaming path moves last, once the boundary is
battle-tested.

### Phase 0: Merge, then burn down the real unknowns

Step zero: merge `nemotron-rust` into `hn-prep` (after the planned
instrumented real-mic run), so the refactor starts from the union: the
Nemotron engine, speaker merge-by-rename, PerfMonitor, and Qwen device-tier
defaults all land in the shipping app.

Already proven — do not respike (evidence in repo):

- ~~Rust/WASM browser inference on a real model~~: `nemotron-asr`, RTF
  0.28–0.5× browser, TTFT 0.65 s, 100% golden accuracy, validated 2026-06-04.
- ~~CPU vs WebGPU for the Rust path~~: answered in `reference/FINDINGS.md`.

Remaining spikes:

- **S1 — TitaNet via rust+ort-web**: embedder in Rust matching the JS
  reference at cosine 1.000000 against the `eval/` fixtures.
- **S2 — JS-host adapter round-trip**: Rust policy driving a transformers.js
  worker through the command protocol. Gate: no measurable latency regression
  on the hot audio path versus the current direct JS loop.
- **S3 — Typed boundary**: ts-rs/tsify generation from `#[non_exhaustive]`
  event enums; generated `.d.ts` compiles against the existing UI.
- **S4 — Storage migration**: IndexedDB from wasm (`indexed_db_futures`),
  opening and migrating a real captured Dexie v2 `SilentNotetaker` database
  with zero data loss.
- **S5 — CI browser-wasm tests** with vendored ort-web runtime assets.
- **S6 — License verification** for every registry default (NVIDIA Nemotron,
  NVIDIA TitaNet, Voxtral, Qwen, SenseVoice) → `license_verified: true` or a
  documented blocker.

Exit criteria: every spike has evidence or is explicitly marked
broken/blocked; the SenseVoice first-party re-host repo exists with pinned,
hashed artifacts (or the blocker is documented).

### Phase 1: Workspace, contracts, and gates

Deliverables: workspace layout; `silent-core` command/event/error/registry
types; full registry with every current model pinned (revision + sha256 —
including re-homing the unpinned TitaNet, `__NEMOTRON_MODEL_BASE`, and
SenseVoice Space-path artifacts to first-party pinned repos); `xtask` (model audit, CSP/`_headers` generation, deploy gate);
CI green including clippy `-D warnings`; the `nemotron-asr` chunking-loop
consolidation. App behavior unchanged.

Exit: core tests pass without a browser; model audit fails on committed
weights (then `titanet.onnx`/`mel_fb.json` are removed from the repo); CSP is
generated and matches what ships today; parity green.

### Phase 2: Diarization in Rust — the first user-visible win

Rust's first contribution is a quality improvement in the product's weakest
area, not a parity grind.

Deliverables: TitaNet embedder on rust+ort-web (S1 productionized); the
SpeakerTracker port (centroid clustering, 8-color rotation, thresholds as
config); **stop-time global recluster** from `docs/DIARIZATION.md` — a new
capability; rename and merge-by-rename as Rust policy with persistence.

Exit: cosine goldens hold; recluster measurably improves labels on the messy
meeting fixtures; manual rename survives recluster; no raw embeddings cross
extension or network boundaries; parity green (speaker labels, legend,
visibility toggle, rename UX unchanged).

### Phase 3: Notes, questions, and corrections policy in Rust

Deliverables: `NoteExtractor` trigger policy ported with goldens captured
from the current regexes (decisions, actions, key points, open questions —
identical behavior first, improvements after); open-question tracking;
word-correction application; smart-question scheduling (type rotation,
reroll, recap) as Rust policy driving the Qwen worker through the typed
boundary; Qwen chunking/dedup/`TAG|` parsing moves from JS to Rust; the
Nemotron adapter migrates from `nemotron-engine.js` glue to the
`silent-web` typed event boundary; **notes-off mode formalized** (R3).

Exit: byte-identical notes on golden transcripts (or documented, reviewed
improvements); disabling Qwen yields the clean transcript-only experience;
teleprompter behavior unchanged; parity green.

### Phase 4: Orchestrator and storage

Deliverables: the recording-session state machine in Rust (start, stop,
resume/continue, new meeting, source tracking, timers); `silent-storage` with
the Dexie v2 migration (S4 productionized) covering all four tables
(meetings, transcriptChunks, notes, screenshots); meeting history + fuzzy
search in Rust; export formatting (notes Markdown, transcript text, summary,
history replay export) and the three timestamp modes as Rust policy.

Exit: **existing users' meetings survive the upgrade** — migration tested
against a real captured database, with export-backup offered before
migration; history search results match current behavior; parity green.

### Phase 5: Engines on Rust law

Deliverables: Voxtral's token/audio two-cap recycle as a unit-tested Rust
policy module driving the js-host (S2 productionized); Whisper, Moonshine,
and Dual through the same adapter; engine selection + device-tier defaults
fully registry-driven; crash diagnostics formalized on `tracing` (ring
buffer, `dumpDiag()`, prior-trail recovery preserved); the model picker now
sources from the registry — adding a model in an existing family is a data
change. `ort` RC-pin upgrade checkpoint.

Exit: 10-minute Voxtral run with flat memory under the Rust policy; engine
swap matrix (every engine × supported tiers) browser-tested; Nemotron on
Firefox/Safari acceptance row green; parity green.

### Phase 6: Extension SDK

Deliverables: manifest schema in Rust; capability enforcement; worker or
sandboxed-iframe host; reference extension; CSP moves from report-only to
enforced.

Exit: reference extension reads approved transcript/notes and exports
Markdown; cannot read raw audio or embeddings; undeclared network access is
blocked and logged.

### Phase 7: Hardening, reproducibility, and publishing

Deliverables: reproducible builds with the in-app wasm hash (R8); the
vendoring decision executed (R6); `docs/ARCHITECTURE.md` rewritten for the
hybrid architecture (retiring the no-build-step claim honestly);
`nemotron-asr` published to crates.io with docs, benchmarks, CHANGELOG;
contributor guide for the Rust workflow; security policy and privacy-boundary
reporting docs; license registry surfaced in-app.

Exit: fresh clone builds, runs, and reproduces the deployed wasm hash; fresh
Cloudflare deploy passes all gates; the crate is live with docs.rs green;
the JS that remains is exactly the permanent-modules list in the workspace
layout — nothing else.

## Open source requirements

- MIT code license remains acceptable unless dependencies force a change
  (`nemotron-asr` is MIT OR Apache-2.0 — the Rust convention — and keeps it).
- Model licenses are not collapsed into the code license. Each model keeps its
  upstream license, visible in the registry and the app.
- Build scripts are readable and deterministic; the build is reproducible
  (R8).
- No generated attribution footers in commits, PRs, or docs.
- The README tells users exactly what origins the app talks to and why,
  generated from the egress manifest.
- The repo includes a security policy explaining privacy-boundary reports.

## Key risks

| Risk | Impact | Required mitigation |
|---|---|---|
| `ort =2.0.0-rc.12` is a release-candidate pin | Breaking changes at GA stall the wasm path | Pin hard now; upgrade checkpoint scheduled at Phase 5. |
| `ort-web 0.2.x` is young with a small maintainer pool | Critical-path dependency stalls | Pin + vendor runtime assets + contribute fixes upstream. |
| JS-host command round-trip adds hot-path latency | Streaming regression vs current direct JS loop | Phase 0 S2 is a measured gate, not a demo. |
| Dexie→Rust storage migration loses meetings | Trust failure for exactly the users we serve | Migration tests on real captured DBs; export-backup before migrate; Phase 4 exit criterion. |
| Duplicate chunking loops (`streaming.rs` vs `backend_web.rs`) diverge | Silent correctness drift between native and wasm | Consolidation is an explicit Phase 1 deliverable. |
| Hugging Face URLs drift (TitaNet, Nemotron base, and SenseVoice's Space path are unpinned today) | App breaks on fresh load | Registry pins revisions and verifies hashes; re-host to first-party repos; stale links fail loudly. |
| Cloudflare Pages 25 MB/file limit vs vendored ort wasm | Deploy fails or vendoring blocked | `xtask` deploy gate checks size; vendoring evaluation accounts for it. |
| Feature regression during strangler-fig migration | Product regression | Appendix A parity check per phase + command-log replay for streaming repros. |
| WebGPU memory behavior under long Voxtral sessions | Long meetings freeze | 10-minute (later 60-minute) memory tests; DEVICE-LOST detection preserved. |
| Extension sandbox weakens privacy claim | Product trust failure | No third-party extensions until CSP enforced + capability checks real. |
| Contributors add large weights by accident | Repo bloat and license risk | CI model audit and Git hooks. |

## Claude review checklist

Claude should review the plan and implementation against these questions:

- Does every phase ship inside the deployed app with the Appendix A parity
  check green — no parallel rebuild, no feature loss?
- Is all policy in Rust with deterministic tests, and are JS adapters
  verifiably free of policy?
- Are mocks, compatibility shims, and unvalidated model paths excluded from
  acceptance?
- Are model weights out of the repo, with pinned revisions and verified
  hashes — including the currently-unpinned TitaNet, Nemotron, and SenseVoice
  sources?
- Is the registry the single source of truth (selection, CSP, egress, licenses,
  budgets), with CSP generated rather than hand-maintained?
- Are the engine contracts implementable in a browser (async-first), with a
  named dispatch strategy?
- Are perf budgets enforced as gates at the measured baselines?
- Are browser-only behaviors validated in a real browser, including the
  Firefox/Safari CPU-tier row?
- Are open source users given enough to audit privacy, licenses, deployment,
  and to reproduce the deployed wasm hash?

## Definition of done

The refactor is done only when the hosted Cloudflare app and a fresh local
clone run the hybrid app such that:

- Every feature in Appendix A works, browser-proven, with the UI unchanged.
- All application policy lives in Rust with deterministic tests; the remaining
  JS is exactly the permanent-modules list.
- Models are swappable per R3 — including Voxtral at the accuracy ceiling,
  Nemotron on CPU-only hardware in Firefox/Safari, and notes-off
  transcript-only mode.
- The registry is pinned and weight-free, the egress manifest and generated
  CSP gates pass, and existing users' stored meetings survive the upgrade.
- Performance gates hold at the R9 baselines.
- The build is reproducible and the running wasm hash is verifiable in-app.
- `nemotron-asr` is published on crates.io with docs and benchmarks.

Anything short of that is a milestone, not the completed refactor.

---

## Appendix A: Feature parity contract

Every row is KEEP. Line anchors are `index.html` on `hn-prep` at the time of
writing (6,283 lines) and will drift; the feature names are the contract.
"Policy owner" is where the behavior's logic lands; rendering stays in the
unchanged UI.

| # | Feature | Today | Policy owner after refactor | Phase |
|---|---|---|---|---|
| 1 | Start/Stop recording, timer, recording dot | index.html:3715-3864 | silent-core orchestrator | 4 |
| 2 | Continue/resume recording without model reload | index.html:3734-3768 | silent-core orchestrator | 4 |
| 3 | New-meeting reset; auto date/time title; 120-char title input | index.html:1375-1380, 6051-6055 | silent-core orchestrator | 4 |
| 4 | Mic capture @16 kHz mono (echo cancel, noise suppress, AGC) | index.html:2809-2950 | capture.js (permanent JS) → typed AudioChunk events | 1 |
| 5 | Tab/system audio capture + dual-channel worklet mix; stream-ended handling | index.html:3429-3476, 3968-4007 | capture.js (permanent JS) | 1 |
| 6 | Sources indicator (Mic / Tab Audio badges) | index.html:1541-1544 | UI, fed by orchestrator events | 4 |
| 7 | ASR engine picker: Voxtral, Dual, SenseVoice, Whisper large-turbo/small/base/tiny, Moonshine (+ Nemotron post-merge) | index.html:5904-5908 | registry-driven selection in silent-inference | 5 |
| 8 | Precision (fp32/fp16/q8/q4) and backend (wasm/webgpu) settings | index.html:5912-5926 | registry device-tier data | 5 |
| 9 | Model download progress + engine status display | index.html:1423-1428, 3078-3082 | EngineEvent::LoadProgress from silent-inference | 3 |
| 10 | Voxtral streaming with in-place partial text and two-cap context recycle | index.html:3037-3108, 4044-4046 | recycle policy in silent-inference (Rust), js-host executes | 5 |
| 11 | Dual mode: Moonshine instant drafts + SenseVoice refined pass; SenseVoice solo (30 s window segmentation) | index.html:2797-3014 | silent-inference draft/refine policy; js-sherpa host | 5 |
| 12 | Speaker diarization (TitaNet embeddings, cosine clustering, 8-color rotation) | index.html:1978-2058, 2942-2945 | silent-diarization | 2 |
| 13 | Speaker labels per line; legend chips; show/hide toggle | index.html:1296-1311, 1520-1522, 4767-4781 | silent-diarization events; UI renders | 2 |
| 14 | Speaker rename (click-to-edit) + persistence + merge-by-rename (nemotron-rust) | index.html:5783-5828 | silent-diarization rename/merge policy | 2 |
| 15 | Stop-time global recluster (docs/DIARIZATION.md) — new in this refactor | — | silent-diarization | 2 |
| 16 | Live trigger notes: decisions, actions, key points, open questions + live counters | index.html:1437-1474, 2527-2580 | silent-notes NoteExtractor | 3 |
| 17 | Note edit / recategorize / delete, persisted | index.html:4280-4357 | silent-storage + silent-notes | 3-4 |
| 18 | Trigger-detection toggle | index.html:5944-5945 | silent-notes config | 3 |
| 19 | AI final notes at Stop (Qwen, chunking ~500 chars, up to 22 chunks, dedup, TAG-format parsing; additive to live notes) | index.html:4570-4610, 2405-2415 | silent-notes NotesEngine policy; js-host executes Qwen | 3 |
| 20 | Notes model selection incl. Qwen3-0.6B/1.7B device-tier auto-default (nemotron-rust) and **off = transcript-only mode** | settings | registry + silent-notes | 3 |
| 21 | Smart-questions teleprompter: Ask Now bar, minimize state, new-question badge, reroll, type selection (clarify/risk/followup), stop-time recap | index.html:1486-1499, 2194-2277, 4527-4568 | silent-notes QuestionGenerator policy; question-worker executes | 3 |
| 22 | Question worker protocol (off-thread WASM, thinking disabled, 64 max tokens) | question-worker.js | typed command protocol from silent-web | 3 |
| 23 | Live transcript rendering: draft/live/final states, word count | index.html:3166-3181, 4091-4107 | EngineEvents; UI renders | 3 |
| 24 | Timestamps: elapsed/clock/ago modes, cycle button, visibility toggle, per-second updates | index.html:4381-4395, 4743-4791 | silent-core formatting policy | 4 |
| 25 | Word corrections: panel, live application via config, persistence across chunks | index.html:1511-1519, 1665, 3529-3535 | silent-notes correction policy | 3 |
| 26 | Screenshots: 15 s periodic capture from tab video, thumbnail strip, IndexedDB storage, toggle | index.html:2724-2792, 4612-4623 | capture.js (permanent JS) + silent-storage | 4 |
| 27 | Screenshot analysis via Claude bridge → key-point note | index.html:3599-3648 | bridge-client.js (permanent JS) + silent-notes | 4 |
| 28 | Claude bridge: WS connect/auto-reconnect/backoff, status dot, setup panel, inline bridge.py download, CLI/API backends, transcript batch analysis, summaries, ad-hoc queries | index.html:1548-1598, 3520-3561; bridge.py | bridge.py + bridge-client.js (permanent); reconnect policy in Rust | 4 |
| 29 | Meeting history: last-50 list, fuzzy search (title/notes/transcript), detail replay | index.html:4806-4929 | silent-storage | 4 |
| 30 | Exports: notes Markdown, transcript text (timestamp-aware), summary copy (incl. AI notes), history replay export, clipboard-fallback modal | index.html:4681-4738, 4931-4953 | silent-core Exporter | 4 |
| 31 | Auto-summary modal at Stop | settings `autoSummary` | silent-core orchestrator | 4 |
| 32 | Settings modal, 15 persisted keys in localStorage `silentNotetaker_settings` | index.html:5860-5999 | silent-core config types; storage unchanged or migrated with compat | 4 |
| 33 | Storage: Dexie v2 `SilentNotetaker` (meetings, transcriptChunks, notes, screenshots) | index.html:1967-1973 | silent-storage with zero-loss migration | 4 |
| 34 | Crash diagnostics: 3 s sampling ring buffer (200 rows, localStorage `notetakerDiag`), heap/ctxLen/recycle/step-time, `dumpDiag()`, DEVICE-LOST detection, prior-trail surfacing on load | index.html:1851-1953, 6065-6086 | tracing-based Diag in silent-core; same UX | 5 |
| 35 | PerfMonitor (TTFT/RTF/chunk-ms; nemotron-rust) | nemotron-rust | EngineStats + tracing | 3 |
| 36 | Toasts, modal system, empty states, note/word count bar | index.html:4668-4676, 5047-5050 | UI (unchanged) | — |

## Appendix B: Registry entry sketches

```toml
[[model]]
id = "asr.nemotron.streaming_0_6b"
task = "asr"
provider = "huggingface"
repo = "<pinned artifact repo>"          # Phase 1: pin the current
revision = "<commit sha>"                 #   __NEMOTRON_MODEL_BASE source
host = "rust-ort-web"
execution_provider = "cpu"                # FINDINGS.md: CPU beats WebGPU here
precision = ["int8"]
memory_budget_mb = 1400
cache = "cache-api"
license = "nvidia-open-model-license"
license_verified = false                  # Phase 0 S6 flips this
network_origins = ["https://huggingface.co", "https://cdn-lfs.huggingface.co"]
  [[model.files]]
  path = "encoder.onnx"        # INT8, ~881 MB
  sha256 = "..."
  [[model.files]]
  path = "decoder_joint_fp32.onnx"   # ~36 MB
  sha256 = "..."
  [[model.files]]
  path = "tokenizer.model"     # ~251 KB
  sha256 = "..."
  [model.device_tiers]
  wasm_only = { default_for_tier = true }
  webgpu_low = { default_for_tier = true }

[[model]]
id = "asr.voxtral.realtime_4b"
task = "asr"
provider = "huggingface"
repo = "onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX"
revision = "<commit sha>"
host = "js-transformers"
execution_provider = "webgpu"
precision = ["q4f16"]
memory_budget_mb = 5500
cache = "transformers-idb"
license = "<verify>"
license_verified = false
  [model.device_tiers]
  webgpu_high = { default_for_tier = true, min_memory_gb = 16 }
  # multi-file artifact list (~2.7 GB) enumerated with per-file sha256;
  # verified once per revision, recorded, not re-hashed per load
```

## Appendix C: Decision log

| Decision | Date | Rationale / evidence |
|---|---|---|
| Hybrid runtime (Rust policy, two hosts) over pure Rust | 2026-06-04 | Voxtral's model class exists only in transformers.js; porting it is months of work for zero user value. `reference/FINDINGS.md`: CPU beat WebGPU for Nemotron's class. Product decision: keep Voxtral as the accuracy ceiling. |
| Voxtral retained as premium tier | 2026-06-04 | Highest-performing model; "4B realtime in a browser tab" headline. Users with strong hardware choose it; others choose Nemotron/Whisper tiers. |
| Swappable models as user-facing product feature | 2026-06-04 | Accuracy ↔ hardware is a user decision, not an engineering default. |
| Notes model optional + extensible slot | 2026-06-04 | Some users don't want LLM notes; transcript-only is a supported mode. Slot stays open for future small models beyond Qwen. |
| Raw `wgpu` spike cut | 2026-06-04 | No model in the lineup consumes it; WebGPU arrives via transformers.js and ort-web EPs. Future note for custom kernels only. |
| SenseVoice kept; artifacts re-hosted first-party | 2026-06-04 | Little/no overhead to keep: copy the sherpa-onnx harness + model out of the k2-fsa Space into a first-party HF repo (as with TitaNet), pin + hash. Also load-bearing: Dual mode's refiner is SenseVoice. |
| ~~COEP `credentialless`~~ (SUPERSEDED 2026-06-05) | 2026-06-04 | ~~Working config today; `require-corp` breaks HF CDN fetches.~~ Superseded by the row below — the "breaks HF CDN" premise was empirically disproven. |
| COEP `require-corp` (supersedes `credentialless`) | 2026-06-05 | Switched to `require-corp`. The 2026-06-04 "breaks HF CDN fetches" assessment was wrong: `docs/research/spike-coep.md` proves HF CDN satisfies `require-corp` via its CORS headers (a CORS-eligible response is CORP-equivalent under the COEP spec — the browser validates the CORS handshake, not a CORP header, for `fetch()` from a cross-origin-isolated context), and transformers.js sends no explicit fetch mode (browser defaults to `cors`). Decisive driver: `require-corp` is the ONLY value WebKit/Safari honors for cross-origin isolation — under `credentialless`, Safari reported `crossOriginIsolated=false` and ran single-threaded. The spike confirmed `crossOriginIsolated=true` + `SharedArrayBuffer` in Chrome, Firefox, AND WebKit under `require-corp`, with every real fetch path (HF TitaNet, jsdelivr transformers.js, vendored same-origin ort-web) passing; the only blocked mode is `fetch(..., {mode:'no-cors'})` (opaque responses), which the app never uses. Closes the R1 Safari blocker. `xtask gen-headers --coep credentialless` is retained as a rollback. INVARIANT: cross-origin fetches must remain CORS-eligible (no `no-cors`). |
| Hosted CSP keeps `ws://localhost:8765` | 2026-06-04 | v1 would have dropped the bridge from hosted builds — a feature loss. Localhost is the user's own machine, inside the trust boundary. Confirmed by Mike. |
| Codex bridge backend deferred to post-refactor | 2026-06-04 | Bridge protocol stays backend-agnostic; Claude (CLI/API) ships now, Codex and other local agent CLIs can slot in later. |
| Mid-recording engine switch queues for next meeting | 2026-06-04 | Friendlier than a rejection: clear "takes effect next meeting" or refresh notice. |
| Strangler-fig migration, no parallel app | 2026-06-04 | `nemotron-engine.js` proved engine-swap-under-unchanged-UI; parallel rewrites are the historically risky path. |
| Edition 2024 for new crates; MSRV = stable−2 | 2026-06-04 | `nemotron-asr` migrates from 2021 when it joins the workspace. |
