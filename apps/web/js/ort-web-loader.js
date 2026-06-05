/**
 * ort-web-loader.js — permanent JS module (PRD R2: JS keeps the "hands").
 *
 * Role: onnxruntime-web ("ort-web") runtime glue for the `rust-ort-web` host
 * (Nemotron, TitaNet). Wires the ort-web loader to vendored, same-origin
 * runtime assets so the app never fetches from `cdn.pyke.io` and never hits the
 * `signal.pyke.io` telemetry beacon (privacy boundary, PRD R5/R6; vendoring
 * procedure in docs/research/spike-ci-wasm.md and proven in spike-titanet.md).
 * It owns NO policy — it only loads and configures the runtime.
 *
 * The `raiseOrtWasmThreads` helper was previously inlined in nemotron-engine.js.
 * It lives here so any future ort-web host (TitaNet, Whisper-ort, …) can import
 * the same thread-count trap without duplicating it.
 *
 * No vendor assets are committed yet (Task z5 / docs/research/spike-ci-wasm.md).
 * When vendoring lands, the asset base-URL configuration belongs here.
 */

/**
 * Raise onnxruntime-web's WASM thread count before the runtime initialises its
 * first session (which allocates the thread pool at the count fixed at that
 * moment). ort-web defaults to `min(4, ceil(hardwareConcurrency/2))` — 4 on a
 * 10-core M1 Pro, leaving half the performance cores idle under the INT8
 * encoder. The `ort` global is created by ort-web's CDN script INSIDE the wasm
 * session build, so we trap the global's assignment and apply the count on the
 * setter.
 *
 * Threads require SharedArrayBuffer, so this is a no-op without
 * cross-origin isolation — ort falls back to 1 thread anyway. The property
 * descriptor seeds a benign empty object value so ort-web's loader, which probes
 * `window.ort[initSymbol]` whenever the property exists, finds a defined (not
 * accessor-undefined) value and does not throw.
 *
 * @param {number} desired  Target thread count (≥2 to have any effect).
 */
export function raiseOrtWasmThreads(desired) {
  if (typeof window === 'undefined' || !window.crossOriginIsolated || !(desired > 1)) return;
  const apply = (ort) => {
    if (!ort) return;
    let tries = 0;
    const tick = () => {
      try { if (ort.env && ort.env.wasm) { ort.env.wasm.numThreads = desired; return; } } catch (_) { return; }
      if (++tries < 100) setTimeout(tick, 10);
    };
    tick();
  };
  if (window.ort) { apply(window.ort); return; }
  // Seed with a benign empty object: ort-web's loader probes `window.ort[initSymbol]`
  // whenever the property exists, and an accessor returning undefined would make that
  // probe throw (it wedges load() at "Building sessions").
  let val = {};
  try {
    Object.defineProperty(window, 'ort', {
      configurable: true,
      enumerable: true,
      get() { return val; },
      set(v) { val = v; apply(v); },
    });
  } catch (_) { /* defineProperty refused — live with ort's default thread count */ }
}
