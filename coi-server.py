#!/usr/bin/env python3
"""Dev server that enables cross-origin isolation (→ SharedArrayBuffer → multithreaded WASM).

COEP=require-corp (switched from credentialless 2026-06-05). require-corp is the only
value WebKit/Safari honors for cross-origin isolation; the spike
(docs/research/spike-coep.md) proved cross-origin CDN imports (jsdelivr/pyke/HF) still
load under require-corp because their CORS headers are CORP-equivalent under the COEP
spec. INVARIANT: cross-origin fetches must
stay CORS-eligible (no no-cors mode). Keep this COEP value byte-identical with
`_headers` and `server/src/main.rs`.
"""
import http.server, socketserver, sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8099


# CSP — ENFORCED, kept byte-identical to the directive set in `_headers`
#   (source of truth: `xtask gen-headers`) and `server/src/main.rs`, so the
#   fallback server exercises the SAME egress policy a hosted deploy enforces.
#   A CSP regression that would bite on Cloudflare Pages should also bite here.
# ws://localhost:8765 (Claude bridge) is included; per the 2026-06-04 decision
#   log it is also KEPT in the hosted `_headers` (localhost is inside the
#   user's trust boundary).
# Note: this fallback server has no `/ext/<name>/` route (extensions need the
#   Rust server); `frame-src 'self'` is kept anyway for parity.
# Rollback: rename the header below to 'Content-Security-Policy-Report-Only'
#   to observe violations in DevTools without blocking.
_CSP = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob: "
        "https://cdn.jsdelivr.net https://cdn.pyke.io; "
    "worker-src 'self' blob:; "
    "connect-src 'self' blob: data: https://cdn.jsdelivr.net https://cdn.pyke.io "
        "https://huggingface.co https://*.hf.co https://cdn-lfs.huggingface.co "
        "https://cdn-lfs-us-1.huggingface.co ws://localhost:8765; "
    "frame-src 'self'; "
    "img-src 'self' data: blob:; "
    "media-src 'self' blob:; "
    "style-src 'self' 'unsafe-inline'"
)

class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
        self.send_header('Cache-Control', 'no-cache')
        # Enforced CSP — same policy as `_headers` / the Rust server. See the
        # comment block above for the parity invariant + rollback procedure.
        self.send_header('Content-Security-Policy', _CSP)
        super().end_headers()

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("", PORT), Handler) as httpd:
    print(f"COI server on :{PORT} (COOP=same-origin, COEP=require-corp)")
    httpd.serve_forever()
