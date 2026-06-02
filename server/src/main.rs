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

    // CSP REPORT-ONLY — for tuning, NOT enforcement.
    // Purpose: observe the real egress surface before locking it down.
    // How to read violations: open DevTools → Console; violations appear as
    //   "[Report Only] Refused to connect to ..." messages.  Note any origins
    //   not in connect-src (especially HF redirect CDN hosts) and add them
    //   before promoting.
    // How to promote to enforcing: once a browser test (with mic + real model
    //   download) shows ZERO violations, rename the header name below from
    //   "content-security-policy-report-only" to "content-security-policy".
    //   Never enforce without that test — blob: worker and HF CDN redirects
    //   can vary and a wrong enforce would block transcription.
    // ws://localhost:8765 is included here (local dev server — Claude bridge
    //   is available locally; it is intentionally absent from the hosted
    //   _headers where the bridge is not available).
    let csp_ro = HeaderName::from_static("content-security-policy-report-only");
    let csp_ro_value = HeaderValue::from_static(
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' blob: https://cdn.jsdelivr.net https://unpkg.com; \
         worker-src 'self' blob:; \
         connect-src 'self' blob: data: https://cdn.jsdelivr.net https://unpkg.com \
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
        // Report-only CSP — observe egress violations without blocking anything.
        // See comment block above for tuning and promotion instructions.
        .layer(SetResponseHeaderLayer::overriding(csp_ro, csp_ro_value))
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
