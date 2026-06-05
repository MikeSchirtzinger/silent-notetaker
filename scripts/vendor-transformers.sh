#!/usr/bin/env bash
# scripts/vendor-transformers.sh — Download + verify the transformers.js runtimes
# and their onnxruntime-web WASM backends for same-origin serving (PRD R6 / K2).
#
# WHY THIS EXISTS (the R6 privacy tightening):
#   The app loads two pinned transformers.js builds from cdn.jsdelivr.net, and each
#   pulls its onnxruntime-web WASM backend from a CDN at runtime. Vendoring all of
#   it same-origin lets the deploy CSP drop cdn.jsdelivr.net / unpkg / cdn.pyke.io
#   from connect-src + script-src — shrinking the egress allowlist to
#   `'self' + Hugging Face + ws://localhost:8765`. That is a REAL tightening of the
#   privacy boundary (the extension-sandbox floor): with the runtimes same-origin,
#   the only third-party origins the page may talk to are HF (model weights) and
#   the user's own localhost bridge.
#
#   z2 made COEP `require-corp`. Same-origin assets trivially satisfy require-corp
#   (no CORS/CORP handshake needed for same-origin), so vendoring also removes the
#   last cross-origin runtime fetch from the cross-origin-isolated context.
#
# NOT COMMITTED: like scripts/vendor-ort-web.sh, this fetches at build/deploy time
# and verifies by sha256. Binaries are never committed to git (the /vendor/ tree is
# gitignored). The deploy bundle is assembled by deploy-cloudflare.sh, which calls
# this script to populate dist/vendor/.
#
# Usage (from repo root):
#   ./scripts/vendor-transformers.sh [output-dir]
#
# Default output dir: <repo_root>/vendor/transformers-runtime
#   (deploy-cloudflare.sh points it at dist/vendor instead.)
#
# Layout produced (matches the loader paths in *-engine.js / index.html):
#   <out>/transformers/3.8.1/transformers.min.js
#   <out>/transformers/3.8.1/ort-wasm-simd-threaded.jsep.{wasm,mjs}   (v3 bundles ORT in its own dist)
#   <out>/transformers/4.0.0-next.7/transformers.min.js
#   <out>/onnxruntime-web/1.25/ort-wasm-simd-threaded.{asyncify,jsep,}.{wasm,mjs}  (v4 fetches ORT 1.25)
#
# The pyke ort-web runtime (Nemotron + TitaNet) is vendored separately by
# scripts/vendor-ort-web.sh into <out>/ort-web/1.24.3/.
#
# Asset hashes verified 2026-06-05 (Task K2 vendoring decision). Every file is
# under Cloudflare Pages' 25 MiB/file limit (largest = ort 1.25 jsep.wasm @ 23.93
# MiB). Update the PIN versions + EXPECTED_HASHES together when bumping a runtime.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${1:-${REPO_ROOT}/vendor/transformers-runtime}"

# --- Pinned versions (must match the import URLs in index.html + *-engine.js) ---
TFJS_V3="3.8.1"                                   # Whisper family (device webgpu→jsep / wasm)
TFJS_V4="4.0.0-next.7"                            # Voxtral + Qwen
ORT_125="1.25.0-dev.20260307-d626b568e0"          # onnxruntime-web pinned by transformers@4.0.0-next.7

JSDELIVR="https://cdn.jsdelivr.net/npm"

# Each row: "<relative-dest-path>|<url>|<sha256>"
# Destination paths are relative to OUTPUT_DIR.
ASSETS=(
  # --- transformers.js v3.8.1: the bundle + the ORT WASM it ships in its own dist ---
  "transformers/${TFJS_V3}/transformers.min.js|${JSDELIVR}/@huggingface/transformers@${TFJS_V3}/dist/transformers.min.js|aa5002b70e789798da263f5f99c62bd3e8fcd0c119258a493c40c180648365fa"
  "transformers/${TFJS_V3}/ort-wasm-simd-threaded.jsep.wasm|${JSDELIVR}/@huggingface/transformers@${TFJS_V3}/dist/ort-wasm-simd-threaded.jsep.wasm|c46655e8a94afc45338d4cb2b840475f88e5012d524509916e505079c00bfa39"
  "transformers/${TFJS_V3}/ort-wasm-simd-threaded.jsep.mjs|${JSDELIVR}/@huggingface/transformers@${TFJS_V3}/dist/ort-wasm-simd-threaded.jsep.mjs|08fb86ec433c78bfb032c5d84a68b8e8e5a8d81268fa39e24314179a5767a5b9"

  # --- transformers.js v4.0.0-next.7: the bundle (ORT comes from onnxruntime-web@1.25) ---
  "transformers/${TFJS_V4}/transformers.min.js|${JSDELIVR}/@huggingface/transformers@${TFJS_V4}/dist/transformers.min.js|83ed9c0680f8664451523abc5d90c7e6c854d8b279f4411895b0dd7d622ba1fc"

  # --- onnxruntime-web 1.25 WASM variants (what transformers@4 selects per browser/device) ---
  # asyncify: Chrome/Firefox device:'wasm'   jsep: device:'webgpu'   plain: Safari/WebKit
  "onnxruntime-web/1.25/ort-wasm-simd-threaded.asyncify.wasm|${JSDELIVR}/onnxruntime-web@${ORT_125}/dist/ort-wasm-simd-threaded.asyncify.wasm|de8f373400e38d4c253f5c6535be22825b733f5238f50bd331427b6ecb872afd"
  "onnxruntime-web/1.25/ort-wasm-simd-threaded.asyncify.mjs|${JSDELIVR}/onnxruntime-web@${ORT_125}/dist/ort-wasm-simd-threaded.asyncify.mjs|b793e8c88697f016ca71c98e773a832db8c6cff8f21a1d84a5dd9791fcfad0f0"
  "onnxruntime-web/1.25/ort-wasm-simd-threaded.jsep.wasm|${JSDELIVR}/onnxruntime-web@${ORT_125}/dist/ort-wasm-simd-threaded.jsep.wasm|66dd6edabc43c9ec1df860978baa403c6610de2f3b3bbfdfcfcbbfadf7677132"
  "onnxruntime-web/1.25/ort-wasm-simd-threaded.jsep.mjs|${JSDELIVR}/onnxruntime-web@${ORT_125}/dist/ort-wasm-simd-threaded.jsep.mjs|ef0fb84f5e1f2fdc5e9d8f6a31c6777979e1718dba49d3ee1e30474d3ebd1689"
  "onnxruntime-web/1.25/ort-wasm-simd-threaded.wasm|${JSDELIVR}/onnxruntime-web@${ORT_125}/dist/ort-wasm-simd-threaded.wasm|76bd6128495e224abffc5a03664f99d4eea1318a8e60ddc06f027d4985087a0c"
  "onnxruntime-web/1.25/ort-wasm-simd-threaded.mjs|${JSDELIVR}/onnxruntime-web@${ORT_125}/dist/ort-wasm-simd-threaded.mjs|7433324767a8aad3f5b90eaac03a2a563973a8d7e5e1ea814d90e08a78ee6820"
)

# Cloudflare Pages per-file limit (25 MiB). Mirrors xtask deploy-gate's
# CF_SIZE_LIMIT_BYTES; this script also fails loudly if a fetched file exceeds it.
CF_LIMIT=$((25 * 1024 * 1024))

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

echo "==> Vendoring transformers.js runtimes to ${OUTPUT_DIR}/"
ALL_OK=1
for row in "${ASSETS[@]}"; do
  dest_rel="${row%%|*}"
  rest="${row#*|}"
  url="${rest%%|*}"
  expected="${rest##*|}"
  dest="${OUTPUT_DIR}/${dest_rel}"
  mkdir -p "$(dirname "$dest")"

  if [[ -f "$dest" ]] && [[ "$(sha256_of "$dest")" == "$expected" ]]; then
    echo "  SKIP: ${dest_rel} (cached, hash matches)"
  else
    echo "  GET:  ${url}"
    curl -fSL --max-time 300 "$url" -o "$dest"
  fi

  actual="$(sha256_of "$dest")"
  size="$(stat -f%z "$dest" 2>/dev/null || stat -c%s "$dest")"
  if [[ "$actual" != "$expected" ]]; then
    echo "  FAIL: ${dest_rel}" >&2
    echo "        expected sha256: ${expected}" >&2
    echo "        got      sha256: ${actual}" >&2
    ALL_OK=0
  elif (( size > CF_LIMIT )); then
    echo "  FAIL: ${dest_rel} is ${size} B — EXCEEDS Cloudflare 25 MiB/file limit" >&2
    ALL_OK=0
  else
    printf "  OK:   %-58s %10d B  sha256:%s…\n" "$dest_rel" "$size" "${actual:0:16}"
  fi
done

if [[ "$ALL_OK" -ne 1 ]]; then
  echo "" >&2
  echo "ERROR: vendoring failed (hash mismatch or oversized file). Do NOT deploy." >&2
  exit 1
fi

echo ""
echo "All transformers.js + onnxruntime-web assets verified under ${OUTPUT_DIR}/"
echo "  transformers.js:  v${TFJS_V3} (Whisper) + v${TFJS_V4} (Voxtral/Qwen)"
echo "  onnxruntime-web:  v1.25 (${ORT_125})"
echo "  CSP impact:       cdn.jsdelivr.net / unpkg / cdn.pyke.io leave connect-src + script-src"
