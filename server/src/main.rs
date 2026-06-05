//! Static file server for the Silent Notetaker.
//!
//! Why this exists instead of `python -m http.server`:
//!   1. Cross-origin isolation (COOP/COEP) → `crossOriginIsolated` → `SharedArrayBuffer`
//!      → multithreaded WASM. Without it, onnxruntime-web / Qwen run single-threaded
//!      (~21s/question vs ~6s at 4 threads).
//!   2. Correct `Content-Type: application/wasm` (via tower-http's mime_guess) so
//!      `WebAssembly.instantiateStreaming` takes the fast streaming-compile path.
//!   3. Range requests (resumable model downloads), no Python runtime dependency,
//!      single self-contained binary for the double-click launcher.
//!
//! Usage: notetaker-server [DIR=.] [PORT=8080]
//!
//! Note: COEP=credentialless (not require-corp) so cross-origin CDN/HF imports still
//! load without every subresource needing a CORP header. WebSocket connections
//! (the Claude bridge on :8765) are not subject to COEP and keep working.

use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    http::{header::CACHE_CONTROL, HeaderName, HeaderValue},
    Router,
};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer, trace::TraceLayer};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(false).init();

    let mut args = env::args().skip(1);
    let dir: PathBuf = args.next().unwrap_or_else(|| ".".to_string()).into();
    let port: u16 = args
        .next()
        .or_else(|| env::var("PORT").ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    // ServeDir handles MIME (application/wasm), range requests, and index.html.
    let serve = ServeDir::new(&dir).append_index_html_on_directories(true);

    let coop = HeaderName::from_static("cross-origin-opener-policy");
    let coep = HeaderName::from_static("cross-origin-embedder-policy");

    // CSP ENFORCED — Phase 6 / R5 (Task j3, "the privacy keystone").
    //
    // SOURCE OF TRUTH: this value MUST stay in sync with the generated policy.
    //   `cargo xtask gen-headers` derives the canonical CSP (shipping `_headers`
    //   + the local-server CSP via `--local-csp-out`) from the model registry +
    //   static invariants. The string below is the same directive set/order that
    //   `gen_headers::generate_local_csp_value` emits (see xtask/src/gen_headers.rs).
    //   If you change egress origins, change them in gen_headers.rs and copy the
    //   regenerated value here — do NOT hand-edit one without the other (that is
    //   exactly the drift this comment exists to prevent: an earlier copy was
    //   missing `https://cdn.pyke.io`, the ort-web onnxruntime-web runtime CDN,
    //   producing spurious violations locally that did not occur in production
    //   where `_headers` is correct).
    //
    // `'wasm-unsafe-eval'` in script-src is REQUIRED: the wasm-pack engines
    //   compile via WebAssembly.instantiateStreaming, which an enforced CSP blocks
    //   without it (it allows WASM compilation only, NOT JS eval). This was a
    //   latent dependency masked by report-only; the j3 enforcement sweep surfaced
    //   it. Fixed in gen_headers.rs (the source of truth) and copied here.
    //
    // ENFORCED (was report-only through Phase 1–5): the regression sweep under
    //   enforcement found ZERO violations from the full app boot + every engine
    //   load + the bridge once `'wasm-unsafe-eval'` was added, so the local dev
    //   server now matches the hosted `_headers` enforcement posture. Egress that
    //   is not in this allowlist is
    //   BLOCKED, not merely reported — that is the privacy boundary R5 promises.
    //   Rollback (re-open the observation period for a regressed origin):
    //   regenerate `_headers` with `--report-only` and rename this header to
    //   "content-security-policy-report-only" in lockstep.
    //
    // Per-extension `connect-src` relaxations come ONLY from the extension's own
    //   sandboxed-iframe CSP (GrantSet::connect_src, applied by extension-host.js
    //   as a <meta> CSP inside the iframe's srcdoc) — never from this BASE page
    //   policy. This base policy carries no extension origins.
    //
    // ws://localhost:8765 (Claude bridge) is included; per the 2026-06-04 decision
    //   log it is also KEPT in the hosted `_headers` (localhost is inside the
    //   user's trust boundary).
    let csp = HeaderName::from_static("content-security-policy");
    // Kept byte-identical to `gen_headers::generate_local_csp_value` output.
    let csp_value = HeaderValue::from_static(
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob: \
             https://cdn.jsdelivr.net https://unpkg.com https://cdn.pyke.io; \
         worker-src 'self' blob:; \
         connect-src 'self' blob: data: \
             https://cdn.jsdelivr.net https://unpkg.com https://cdn.pyke.io \
             https://huggingface.co https://*.hf.co https://cdn-lfs.huggingface.co \
             https://cdn-lfs-us-1.huggingface.co ws://localhost:8765; \
         img-src 'self' data: blob:; \
         media-src 'self' blob:; \
         style-src 'self' 'unsafe-inline'",
    );

    let app = Router::new()
        .fallback_service(serve)
        // Cross-origin isolation → multithreaded WASM.
        .layer(SetResponseHeaderLayer::overriding(
            coop,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            coep,
            HeaderValue::from_static("credentialless"),
        ))
        // Enforced CSP — egress outside the allowlist is BLOCKED (Phase 6 / R5).
        // See comment block above for the source-of-truth + rollback procedure.
        .layer(SetResponseHeaderLayer::overriding(csp, csp_value))
        // Dev: always reflect local edits. The browser's MODEL cache is separate
        // (IndexedDB / Cache API, keyed by origin) and is unaffected by this.
        .layer(SetResponseHeaderLayer::overriding(
            CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let canon = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    tracing::info!("Notetaker server → http://localhost:{port}   serving {}", canon.display());
    tracing::info!("cross-origin isolated (COOP=same-origin, COEP=credentialless) → multithreaded WASM enabled");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr} failed: {e} (port in use?)"));
    axum::serve(listener, app).await.unwrap();
}
