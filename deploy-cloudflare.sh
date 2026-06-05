#!/usr/bin/env bash
# Deploy the Silent Notetaker to Cloudflare Pages.
#
# Cloudflare Pages serves the small static files with cross-origin-isolation
# headers (see _headers). Large model weights (titanet.onnx ~40 MB, Nemotron
# encoder ~881 MB) are NOT deployed here — they live on HuggingFace CDN
# (Pages has a 25 MB per-file limit), loaded at runtime.
#
# Both wasm-pack crates are built at deploy time (NOT committed to git):
#   - crates/nemotron-asr  → dist/crates/nemotron-asr/pkg/
#   - crates/silent-web    → dist/crates/silent-web/pkg/
#
# Free tier: unlimited bandwidth, so heavy usage stays $0.
#
# One-time setup:
#   npm install -g wrangler   # or: brew install cloudflare-wrangler2
#   wrangler login            # opens browser, authorizes your Cloudflare account
#
# Usage:
#   ./deploy-cloudflare.sh              # build + deploy
#   ./deploy-cloudflare.sh --dry-run    # build + gate check, stop before deploy
#
set -euo pipefail
cd "$(dirname "$0")"

DRY_RUN=false
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    *) echo "Unknown argument: $arg"; exit 1 ;;
  esac
done

PROJECT="${CF_PAGES_PROJECT:-silent-notetaker}"
DIST="dist"

# ── 1. Build wasm-pack crates ──────────────────────────────────────────────

echo "▸ Building crates/nemotron-asr (wasm-pack, --target web --release) ..."
wasm-pack build crates/nemotron-asr --target web --release

echo "▸ Building crates/silent-web (wasm-pack, --target web --release) ..."
wasm-pack build crates/silent-web --target web --release

# ── 2. Assemble dist/ ─────────────────────────────────────────────────────

echo "▸ Assembling deploy bundle in ./$DIST ..."
rm -rf "$DIST"
mkdir -p "$DIST"

# App shell + worker
cp index.html "$DIST/"
cp question-worker.js "$DIST/"
cp nemotron-engine.js "$DIST/"
cp diarization-engine.js "$DIST/"
cp notes-engine.js "$DIST/"
cp session-engine.js "$DIST/"
cp storage-engine.js "$DIST/"
cp exports-engine.js "$DIST/"

# Cloudflare Pages response headers (COOP/COEP + CSP)
cp _headers "$DIST/"

# wasm-pack pkg outputs at the paths the engine loaders expect:
#   nemotron-engine.js  → ./crates/nemotron-asr/pkg/nemotron_asr.js
#   diarization-engine.js → ./crates/silent-web/pkg/silent_web.js
mkdir -p "$DIST/crates/nemotron-asr"
cp -r crates/nemotron-asr/pkg "$DIST/crates/nemotron-asr/pkg"

mkdir -p "$DIST/crates/silent-web"
cp -r crates/silent-web/pkg "$DIST/crates/silent-web/pkg"

echo "▸ Bundle contents:"
find "$DIST" -type f | sort
echo ""
du -sh "$DIST"

# ── 3. Deploy gate (R6) ───────────────────────────────────────────────────

echo "▸ Running xtask deploy-gate ..."
cargo run -p xtask -- deploy-gate "$DIST"
echo "✓ deploy-gate PASS"

# ── 4. Deploy to Cloudflare Pages ─────────────────────────────────────────

if [ "$DRY_RUN" = true ]; then
  echo "▸ --dry-run: skipping wrangler deploy. Bundle is ready in ./$DIST"
  exit 0
fi

echo "▸ Deploying to Cloudflare Pages project '$PROJECT' ..."
# Cloudflare environment tokens lack Pages permissions; unset them so wrangler
# falls back to the wrangler login OAuth token.
unset CLOUDFLARE_API_TOKEN CF_API_TOKEN
wrangler pages deploy "$DIST" --project-name "$PROJECT" --commit-dirty=true

echo "✓ Done. Cross-origin isolation is set via _headers (COOP/COEP)."
