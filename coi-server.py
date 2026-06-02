#!/usr/bin/env python3
"""Dev server that enables cross-origin isolation (→ SharedArrayBuffer → multithreaded WASM).

COEP=credentialless lets cross-origin CDN imports (jsdelivr/HF) still load without
needing CORP headers on every subresource. Same as require-corp for isolation purposes.
"""
import http.server, socketserver, sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8099


# CSP REPORT-ONLY — for tuning, NOT enforcement.
# Purpose: observe the real egress surface before locking it down.
# How to read violations: open DevTools → Console; violations appear as
#   "[Report Only] Refused to connect to ..." messages.  Note any origins
#   not in connect-src (especially HF redirect CDN hosts like cdn-lfs-us-1
#   or regional variants) and add them before promoting.
# How to promote to enforcing: once a browser test (with mic + real model
#   download) shows ZERO violations, rename the header below from
#   'Content-Security-Policy-Report-Only' to 'Content-Security-Policy'.
#   Never enforce without that test — blob: worker and HF CDN redirects
#   can vary and a wrong enforce would block transcription.
# ws://localhost:8765 is included here (local dev server — Claude bridge
#   is available locally; it is intentionally absent from the hosted
#   _headers where the bridge is not available).
_CSP_REPORT_ONLY = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline' blob: https://cdn.jsdelivr.net https://unpkg.com; "
    "worker-src 'self' blob:; "
    "connect-src 'self' blob: data: https://cdn.jsdelivr.net https://unpkg.com "
        "https://huggingface.co https://*.hf.co https://cdn-lfs.huggingface.co "
        "https://cdn-lfs-us-1.huggingface.co ws://localhost:8765; "
    "img-src 'self' data: blob:; "
    "media-src 'self' blob:; "
    "style-src 'self' 'unsafe-inline'"
)

class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'credentialless')
        self.send_header('Cache-Control', 'no-cache')
        # Report-only CSP — observe egress violations without blocking anything.
        # See comment block above for tuning and promotion instructions.
        self.send_header('Content-Security-Policy-Report-Only', _CSP_REPORT_ONLY)
        super().end_headers()

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("", PORT), Handler) as httpd:
    print(f"COI server on :{PORT} (COOP=same-origin, COEP=credentialless)")
    httpd.serve_forever()
