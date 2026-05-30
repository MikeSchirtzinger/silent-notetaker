#!/usr/bin/env python3
"""Dev server that enables cross-origin isolation (→ SharedArrayBuffer → multithreaded WASM).

COEP=credentialless lets cross-origin CDN imports (jsdelivr/HF) still load without
needing CORP headers on every subresource. Same as require-corp for isolation purposes.
"""
import http.server, socketserver, sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8099

class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
        self.send_header('Cross-Origin-Embedder-Policy', 'credentialless')
        self.send_header('Cache-Control', 'no-cache')
        super().end_headers()

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("", PORT), Handler) as httpd:
    print(f"COI server on :{PORT} (COOP=same-origin, COEP=credentialless)")
    httpd.serve_forever()
