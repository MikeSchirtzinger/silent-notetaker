/**
 * ort-web-loader.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * STATUS: placeholder (Task C1 scaffold). Not yet loaded by index.html.
 *
 * Role: onnxruntime-web ("ort-web") runtime glue for the `rust-ort-web` host
 * (Nemotron, TitaNet). Wires the ort-web loader to **vendored, same-origin**
 * runtime assets so the app never fetches from `cdn.pyke.io` and never hits the
 * `signal.pyke.io` telemetry beacon (privacy boundary, PRD R5/R6; vendoring
 * procedure in docs/research/spike-ci-wasm.md and proven in spike-titanet.md).
 * It owns NO policy — it only loads and configures the runtime.
 *
 * Today this glue is partly inside nemotron-engine.js (thread-count raising) and
 * the wasm-pack bundle; it consolidates here as the ort-web host generalizes.
 */
export {};
