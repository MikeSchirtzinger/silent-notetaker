/**
 * transformers-host.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * STATUS: placeholder (Task C1 scaffold). Not yet loaded by index.html.
 *
 * Role: the `js-transformers` model host worker. It runs transformers.js models
 * (Voxtral, Whisper family, Moonshine, Qwen) and executes generate/decode steps
 * on command. It contains NO policy: chunk sizes, Voxtral's two-cap recycle,
 * when to feed, and when to recycle are decided in Rust and arrive as typed
 * commands; the worker returns events. Reviewers can verify the absence of
 * policy by reading this file (PRD R2 acceptance).
 *
 * The boundary shape and its negligible hot-path cost are proven in
 * docs/research/spike-jshost.md (GATE: PASS — no measurable latency regression;
 * keep the transferable discipline for audio/tensor payloads). Lands in Phase 5.
 */
export {};
