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

# ── 1. Build wasm-pack crates (reproducible — PRD R8) ──────────────────────
#
# Delegate to scripts/build-wasm.sh so the deployed bytes are the SAME bytes a
# fresh local clone reproduces (remapped source paths → stable hash). The script
# builds both crates and prints the sha256 manifest, which we capture and ship in
# the bundle so the running app can verify hosted == source (settings → About).

echo "▸ Building wasm (reproducible, scripts/build-wasm.sh) ..."
WASM_MANIFEST="$(./scripts/build-wasm.sh | tail -2)"

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
cp bridge-engine.js "$DIST/"
cp selection-engine.js "$DIST/"
# Phase 5 (step y2-engine-paths; Appendix A rows 10, 11): the js-host engine
# loaders that drive the silent_web `WasmWhisperStream`/`WasmVoxtralRecycle`/
# `WasmSenseVoice`/`WasmDual` policies (REPLACED the inline index.html loops).
cp whisper-engine.js "$DIST/"
cp voxtral-engine.js "$DIST/"
cp sensevoice-engine.js "$DIST/"
cp dual-engine.js "$DIST/"
# Phase 5 (step y3-diag-wiring; Appendix A rows 34, 35): the crash-diagnostics +
# PerfMonitor loader that drives the silent_web `WasmDiag` sampler (REPLACED the
# inline index.html `Diag` IIFE + dumpDiag/clearDiag + prior-trail banner).
cp diag-engine.js "$DIST/"
# Phase 6 (Task J2 / R7): the sandboxed-iframe extension host runtime that drives
# the silent_web `extension_host` surface (manifest validation, grant-set
# persistence, the per-extension data/UI/network boundary, the versioned envelope).
cp extension-host.js "$DIST/"
# Phase 6 (R7): the bundled reference extension (manifest + entrypoint). Served as
# static files; each extension loads inside its own null-origin sandboxed iframe.
mkdir -p "$DIST/extensions"
cp -r extensions/reference-notes-export "$DIST/extensions/reference-notes-export"

# Cloudflare Pages response headers (COOP/COEP + CSP)
cp _headers "$DIST/"

# wasm-pack pkg outputs at the paths the engine loaders expect:
#   nemotron-engine.js  → ./crates/nemotron-asr/pkg/nemotron_asr.js
#   diarization-engine.js → ./crates/silent-web/pkg/silent_web.js
mkdir -p "$DIST/crates/nemotron-asr"
cp -r crates/nemotron-asr/pkg "$DIST/crates/nemotron-asr/pkg"

mkdir -p "$DIST/crates/silent-web"
cp -r crates/silent-web/pkg "$DIST/crates/silent-web/pkg"

# ── 2b. wasm hashes manifest (PRD R8) ──────────────────────────────────────
#
# Re-hash the wasm exactly as it sits in the bundle (the bytes that will be
# served) and write the served-relative manifest the running app fetches to
# verify hosted == source. Paths are relative to the deploy root (= index.html's
# base URL), so the app can request `./wasm-hashes.txt` and match each entry
# against the bytes it actually loaded.
echo "▸ Writing wasm-hashes.txt manifest into bundle ..."
sha256_bundle() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
{
  echo "# Silent Notetaker — deployed wasm sha256 manifest (PRD R8)."
  echo "# Reproduce locally: ./scripts/build-wasm.sh   (see README → Verify the binary)."
  echo "# Format: <sha256>  <path-relative-to-app-root>"
  printf '%s  crates/silent-web/pkg/silent_web_bg.wasm\n' \
    "$(sha256_bundle "$DIST/crates/silent-web/pkg/silent_web_bg.wasm")"
  printf '%s  crates/nemotron-asr/pkg/nemotron_asr_bg.wasm\n' \
    "$(sha256_bundle "$DIST/crates/nemotron-asr/pkg/nemotron_asr_bg.wasm")"
} > "$DIST/wasm-hashes.txt"
cat "$DIST/wasm-hashes.txt"

# Sanity: the in-bundle hashes MUST equal what build-wasm.sh just reported. If
# the copy into dist/ ever changed a byte, this catches it before deploy.
echo "▸ Cross-checking bundle hashes against the build manifest ..."
DIST_SW="$(sha256_bundle "$DIST/crates/silent-web/pkg/silent_web_bg.wasm")"
DIST_NE="$(sha256_bundle "$DIST/crates/nemotron-asr/pkg/nemotron_asr_bg.wasm")"
if ! grep -q "$DIST_SW" <<< "$WASM_MANIFEST" || ! grep -q "$DIST_NE" <<< "$WASM_MANIFEST"; then
  echo "✗ Bundle wasm hash does not match the freshly-built artifact. Aborting." >&2
  echo "  build manifest:" >&2; echo "$WASM_MANIFEST" >&2
  echo "  bundle: $DIST_SW  silent_web   $DIST_NE  nemotron_asr" >&2
  exit 1
fi
echo "✓ Bundle wasm == freshly-built wasm."

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
