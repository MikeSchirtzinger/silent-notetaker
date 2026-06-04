/**
 * bridge-client.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * STATUS: placeholder (Task C1 scaffold). Not yet loaded by index.html.
 *
 * Role: a thin WebSocket client for the local Claude bridge (`bridge.py`) over
 * `ws://localhost:8765` — the user's own machine, inside the trust boundary
 * (PRD R5; hosted CSP keeps `ws://localhost:8765` in connect-src). It sends and
 * receives messages only; the reconnect / auto-backoff / status policy and the
 * transcript-batch / summary / screenshot-analysis logic are Rust policy
 * (Appendix A rows 27, 28). The bridge stays backend-agnostic (Claude CLI/API
 * now; Codex and other local agent CLIs are a deferred future option).
 *
 * The bridge logic currently lives inline in index.html; it migrates here in
 * Phase 4 with the reconnect policy moving to Rust.
 */
export {};
