#!/usr/bin/env bash
# Deploy the Silent Notetaker to Cloudflare Pages.
#
# Cloudflare Pages serves the small static files with cross-origin-isolation
# headers (see _headers). The 40MB titanet.onnx is NOT deployed here — it lives
# on the HuggingFace CDN (Pages has a 25MB per-file limit), loaded at runtime.
#
# Free tier: unlimited bandwidth, so heavy usage stays $0.
#
# One-time setup:
#   npm install -g wrangler   # or: brew install cloudflare-wrangler2
#   wrangler login            # opens browser, authorizes your Cloudflare account
#
# Then just run:  ./deploy-cloudflare.sh
set -euo pipefail
cd "$(dirname "$0")"

PROJECT="${CF_PAGES_PROJECT:-silent-notetaker}"
DIST="dist"

echo "▸ Assembling deploy bundle in ./$DIST ..."
rm -rf "$DIST"
mkdir -p "$DIST"
# Only the files the running app actually needs (titanet.onnx is on HF):
cp index.html question-worker.js mel_fb.json _headers "$DIST/"

echo "▸ Bundle contents:"
ls -lh "$DIST"

echo "▸ Deploying to Cloudflare Pages project '$PROJECT' ..."
wrangler pages deploy "$DIST" --project-name "$PROJECT"

echo "✓ Done. Cross-origin isolation is set via _headers (COOP/COEP)."
