/**
 * capture.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * STATUS: placeholder (Task C1 scaffold). Not yet loaded by index.html.
 *
 * Role: browser audio capture only — `getUserMedia` (mic @16 kHz mono with echo
 * cancel / noise suppress / AGC), `getDisplayMedia` (tab/system audio +
 * dual-channel worklet mix, stream-ended handling), and the AudioWorklet graph
 * (Appendix A rows 4, 5, 26). It emits typed `AudioChunk` events to the Rust
 * core; it owns NO policy (chunk sizes, engine selection, and VAD thresholds all
 * arrive from Rust). Screenshot capture (row 26) also lands here.
 *
 * This logic currently lives inline in index.html; it migrates here in Phase 1
 * via the strangler-fig pattern, with the Appendix A parity check green.
 */
export {};
