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
//! Note: COEP=require-corp (switched from credentialless 2026-06-05). require-corp
//! is the only value WebKit/Safari honors for cross-origin isolation; the earlier
//! "credentialless avoids needing CORP on every subresource" reasoning was kept for
//! Safari compatibility, but the spike (docs/research/spike-coep.md) proved the CDN
//! origins (HF, jsdelivr, cdn.pyke.io) satisfy require-corp via their CORS headers
//! (CORS-eligible == CORP-eligible under the COEP spec). WebSocket
//! connections (the Claude bridge on :8765) are not subject to COEP and keep working.
//! INVARIANT: cross-origin fetches must stay CORS-eligible (no no-cors mode).
//! This value MUST stay byte-identical with `_headers` and `coi-server.py`.

mod ext_route;

use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    http::{header::CACHE_CONTROL, HeaderName, HeaderValue},
    routing::get,
    Router,
};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer, trace::TraceLayer};

/// The enforced base-page CSP. Kept byte-identical to
/// `gen_headers::generate_local_csp_value` output — see the comment block at the
/// use site in `main()` for the full rationale, and the `csp_matches_headers_file`
/// test below for the guard that keeps this from drifting out of sync with the
/// shipped `_headers`.
const CSP_VALUE: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob: \
         https://cdn.jsdelivr.net https://cdn.pyke.io; \
     worker-src 'self' blob:; \
     connect-src 'self' blob: data: \
         https://cdn.jsdelivr.net https://cdn.pyke.io \
         https://huggingface.co https://*.hf.co https://cdn-lfs.huggingface.co \
         https://cdn-lfs-us-1.huggingface.co ws://localhost:8765; \
     frame-src 'self'; \
     img-src 'self' data: blob:; \
     media-src 'self' blob:; \
     style-src 'self' 'unsafe-inline'";

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
    //   document, served by the `/ext/<name>/` route (see `ext_route`) with a
    //   per-extension RESPONSE-HEADER CSP built from GrantSet::connect_src(). A
    //   response-header CSP is not inherited by the embedder (unlike the old
    //   srcdoc `<meta>` CSP, which Chromium intersects with the base page — the j3
    //   finding that made grants inert), so a granted origin actually takes effect
    //   there while NEVER touching this BASE page policy. This base policy carries
    //   no extension origins; `frame-src 'self'` authorizes framing that route.
    //
    // ws://localhost:8765 (Claude bridge) is included; per the 2026-06-04 decision
    //   log it is also KEPT in the hosted `_headers` (localhost is inside the
    //   user's trust boundary).
    let csp = HeaderName::from_static("content-security-policy");
    // Kept byte-identical to `gen_headers::generate_local_csp_value` output
    // (guarded by the `csp_matches_headers_file` test).
    let csp_value = HeaderValue::from_static(CSP_VALUE);

    // The static surface (index.html, wasm, models, …) carries the ENFORCED BASE
    // page CSP. The base CSP MUST NOT change per extension; per-extension
    // network grants live ONLY on the /ext/<name>/ route below.
    //
    // Dev cache: ServeDir gets `no-cache` so local edits always reflect. The
    // browser's MODEL cache is separate (IndexedDB / Cache API, keyed by origin).
    let static_app = Router::new()
        .fallback_service(serve)
        // Enforced CSP — egress outside the allowlist is BLOCKED (Phase 6 / R5).
        // See comment block above for the source-of-truth + rollback procedure.
        .layer(SetResponseHeaderLayer::overriding(csp, csp_value))
        .layer(SetResponseHeaderLayer::overriding(
            CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ));

    // The per-extension document route (Task j2b — the network-grant keystone).
    // Each installed extension is served as a DISTINCT same-origin document whose
    // OWN response-header CSP carries exactly its granted `connect-src` (from the
    // sanitized `o=` query params the in-page host derives from
    // `GrantSet::connect_src()`). A response-header CSP is NOT inherited from the
    // embedder (unlike the old `srcdoc`/`<meta>` CSP), so a granted origin
    // actually takes effect — while `sandbox="allow-scripts"` still forces the
    // opaque origin that keeps the extension out of the host's IndexedDB /
    // localStorage. `serve_extension_doc` sets the per-extension CSP + `no-store`;
    // we do NOT apply the base CSP layer here. COOP/COEP ARE applied (below, to
    // the merged router) so the ext doc can be framed in the isolated base page.
    let ext_app = Router::new()
        .route("/ext/{name}/", get(ext_route::serve_extension_doc))
        .route("/ext/{name}", get(ext_route::serve_extension_doc));

    let app = ext_app
        .merge(static_app)
        // Cross-origin isolation → multithreaded WASM. Applied to BOTH surfaces:
        // the base page needs it for SharedArrayBuffer; the ext doc needs
        // require-corp + same-origin COOP so it can be embedded in the isolated
        // base page without breaking `crossOriginIsolated`.
        .layer(SetResponseHeaderLayer::overriding(
            coop,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            coep,
            HeaderValue::from_static("require-corp"),
        ))
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let canon = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    tracing::info!(
        "Notetaker server → http://localhost:{port}   serving {}",
        canon.display()
    );
    tracing::info!(
        "cross-origin isolated (COOP=same-origin, COEP=require-corp) → multithreaded WASM enabled"
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr} failed: {e} (port in use?)"));
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::CSP_VALUE;

    /// Parity guard: the local server's enforced CSP must stay byte-identical to
    /// the `Content-Security-Policy` line in the shipped `_headers` (both are
    /// derived from `cargo xtask gen-headers`). This is exactly the drift the
    /// comment in `main()` warns about — an earlier copy silently diverged
    /// (missing `cdn.pyke.io`, later a stale `unpkg.com`); this test makes the
    /// next divergence a test failure instead of a "works locally, differs
    /// hosted" surprise.
    #[test]
    fn csp_matches_headers_file() {
        let headers_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../_headers");
        let headers = std::fs::read_to_string(headers_path)
            .expect("read ../_headers (run from the repo checkout)");
        let from_headers = headers
            .lines()
            .find_map(|l| l.strip_prefix("Content-Security-Policy: "))
            .expect("_headers has a Content-Security-Policy line");
        assert_eq!(
            CSP_VALUE, from_headers,
            "server CSP and _headers CSP have drifted — regenerate both from \
             `cargo xtask gen-headers` (see gen_headers.rs)"
        );
    }
}
