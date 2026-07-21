# Show HN — draft

Internal prep doc for the Hacker News launch. Not part of the app.

---

## Strategy in one line

This is a **credibility play**, not a product launch. HN rewards the *engineering*
(a 4B streaming model running in a browser tab, three models on different silicon,
the memory-runaway fix) far more than "another meeting notetaker." Lead with the
hard parts, position on architectural privacy, be ruthlessly honest about the rough
edges, and let the work speak. The payoff is reputation + an audience, not signups.

---

## Title options (pick one)

HN titles should be specific and technical, no hype, no buzzwords, ideally < 80 chars.

1. **Show HN: I ran a 4B streaming speech model in a browser tab (private meeting notes)** ← recommended
2. Show HN: Silent Notetaker – on-device meeting notes, your audio never leaves the browser
3. Show HN: A meeting notetaker that's private by architecture, not by policy
4. Show HN: Three AI models at once in one browser tab, fully local meeting notes

#1 leads with the technical feat (the thing HN clicks on) and puts the use-case in
parens. Avoid adjectives like "blazing," "powerful," "revolutionary" — HN allergic.

---

## Body draft

> Silent Notetaker is a meeting notetaker that runs entirely in your browser — live
> transcription, speaker labels, and decision/action-item extraction — with the audio
> never leaving your machine. No backend, no account — just static files you can read
> and host yourself (an `index.html` shell, ES-module engine loaders, and a
> hash-verifiable Rust/WASM core).
>
> I built it because every mainstream AI notetaker, including the privacy-marketed
> ones, streams your audio to their servers and runs the AI there. Their privacy is a
> promise. I wanted it to be a property of the architecture instead: the audio is
> captured, fed to the models, and consumed in-process — it's never put on the wire.
> You can verify that in the network panel; the only things it fetches are JS
> libraries and model weights from CDNs.
>
> The interesting part was making three models run at once without choking. By
> default all three — the streaming ASR (NVIDIA's Nemotron, INT8), a speaker-ID model
> (TitaNet), and a small question-suggesting LLM (Qwen3) — run on WASM/CPU, which
> keeps the whole default pipeline off the GPU so nothing contends. There's also an
> optional premium tier: a 4B streaming model (Voxtral) that takes the GPU to itself
> via WebGPU when you select it. That GPU path is where the nastiest bug lived — an
> invisible WebGPU memory runaway: the streaming model's KV cache grows every token in
> GPU memory that the JS heap profiler can't see, so an hour-long meeting silently
> balloons to ~2 GB and freezes the tab. Fixed it by capping the per-context token +
> audio budget and recycling the context, re-anchored at "now" so no audio is dropped.
>
> Honest about the rough edges: it wants a modern Chromium (Chrome/Edge), the first
> load downloads the model weights (a few hundred MB for the default Nemotron, up to
> ~2.7 GB if you opt into the Voxtral GPU tier) — after that they're cached on-device
> in the browser's private file system (OPFS), keyed to the model revision, so repeat
> visits skip the download. Live speaker clustering still over-splits sometimes, and
> it captures your mic rather than system audio today. Lighter engines (SenseVoice,
> Whisper base/small) are in there for weaker machines.
>
> Code is MIT, no build step, audit away: [repo link]
> Hosted demo (purely on-device, nothing to install): [Cloudflare link]

Keep it to roughly this length. Resist adding a feature list — link the README for that.

---

## First comment (post immediately, carries the deep technical detail)

> Author here. A few implementation notes for the curious, since "it runs in a
> browser" is doing a lot of work:
>
> **The invisible memory runaway (the optional Voxtral GPU tier).** The default engine
> (Nemotron) runs on CPU/WASM, but the optional Voxtral 4B tier streams on the GPU —
> and that's where this one lived. Inside one `model.generate()` call the KV cache
> (`past_key_values`) grows with every emitted token, and that memory is GPU/native —
> it does *not* show up in the JS heap profiler, which is where you'd instinctively
> look. Measured ~0.52 MB/token on M1 Metal with real Voxtral 4B; the old
> `max_new_tokens: 4096` peaked near 2 GB and froze the tab around the 5-minute mark.
> The fix is two caps feeding one outer recycle loop: a 320-token budget (~166 MB
> peak) and a 45-second audio-window cap (catches slow/quiet contexts that creep up
> without emitting tokens). When a context returns, a fresh one is anchored at the
> current ring-buffer write position, so it reads forward from "now" — flat memory
> over an arbitrarily long meeting, no gap at the seam.
>
> **Speaker ID from scratch.** There's no off-the-shelf browser diarization pipeline,
> so TitaNet runs via onnxruntime-web and I reimplemented its mel-spectrogram
> front-end in pure JS — then byte-validated it against the reference Python
> (cosine similarity 1.000000) so the embeddings are identical. Speakers are tracked
> by online "leader clustering" on a cosine threshold (~0.45). The live clustering is
> the weakest link — it over-splits — and global re-clustering is what I'm working on.
>
> **Why split GPU vs WASM.** By default everything runs on CPU/WASM — Nemotron ASR,
> TitaNet, and Qwen — so nothing touches the GPU at all. Opt into a GPU ASR tier
> (Voxtral) and the GPU becomes the bottleneck, so that heaviest model owns it and the
> other two stay on CPU/WASM where they can never contend for GPU memory or scheduling.
>
> There's a scrollytelling writeup of the six main build decisions in the repo
> (`overview.html`), and full architecture notes in `docs/ARCHITECTURE.md` — including
> the now-modular core and the shipped sandboxed, network-denied-by-default extension
> system (the groundwork for a future marketplace that can't undermine the privacy
> guarantee; grants land via a per-extension response-header CSP).
>
> Built by Brevity — we build private, on-device AI. Happy to answer anything.

---

## Pre-empt the predictable HN objections (have answers ready)

- **"6,000-line single HTML file is gross."** It's since been split — the monolith is
  now a root `index.html` shell plus native ES-module engine loaders and Rust/WASM
  crates, no build step (Cloudflare serves the modules directly). The single-file era
  was a deliberate auditability/shareability choice for the early demo; "verify the
  binary you're running" (published WASM hashes) replaced it. Point to
  `docs/ARCHITECTURE.md`.
- **"Weights come from Hugging Face, so it's not really offline / not really local."**
  Weights download once and cache; inference is 100% client-side; audio never goes
  anywhere. The egress surface is libraries + weights, full stop — and that's listed
  explicitly in the README so people can check.
- **"Just use a native app (Granola/Char/Meetily)."** Those are great and capture
  system audio (a real advantage this doesn't have yet). The differences: this is
  zero-install, cross-platform (any WebGPU machine, not Mac-only), a single auditable
  file, and private by architecture rather than policy. Native system-audio capture
  via a Tauri shell is on the roadmap.
- **"How is this different from Granola specifically?"** Granola transcribes
  on-device but sends audio to cloud LLMs (GPT-4o/Claude) and stores data on AWS.
  This never sends the audio anywhere. Don't dunk — Granola's UX is excellent; the
  distinction is purely the trust model.
- **"Diarization is bad."** Yes — said so up front. Embeddings are solid (0% EER on
  clean speech in a 6-model bake-off — LibriSpeech test-clean; harness + results in
  `/eval`); live clustering on messy meeting audio over-splits, and that — not the
  embeddings — is the real weak link. Global re-clustering is in progress.

Tone for all replies: factual, generous to competitors, quick to concede real
limitations. HN punishes defensiveness and rewards a builder who knows exactly where
the bodies are buried.

---

## Pre-launch checklist

- [x] Fill in the real **Brevity link / email capture** (README footer → https://brevity.ventures) — this is how the launch converts into a following.
- [x] **CSP added and browser-validated** (the `connect-src` allowlist — turns "audio never leaves" from asserted to enforced; see `docs/ARCHITECTURE.md` §6). Live on silentnotetaker.com as a response header, alongside COOP/COEP.
- [ ] Hosted Cloudflare link tested cold on a fresh profile (first-load model download works on real wifi).
- [ ] README renders correctly on GitHub (tables, the egress-surface table especially).
- [ ] A 30–60s screen recording or GIF in the README/post — HN engagement jumps with a visual.
- [ ] Diarization over-split visibly improved, or at least clearly labeled in the UI.
- [ ] Post early-to-mid week morning US time; be available to answer comments for the first 2–3 hours.
