#!/usr/bin/env bash
# scripts/vendor-ort-web.sh — Download and verify ort-web 1.24.3 runtime assets.
#
# Usage (from repo root):
#   ./scripts/vendor-ort-web.sh [output-dir]
#
# Default output dir: crates/nemotron-asr/vendor/ort-web-1.24.3
#
# The three downloaded files are the FEATURE_NONE (CPU-only) ort-web build.
# They must be served over HTTP for ort_web::Dist::new() to load without CORS
# errors when running wasm-pack browser tests.
#
# Asset hashes verified 2026-06-04 (B3 spike).
# Update ORT_WEB_VERSION + EXPECTED_HASHES together when bumping the runtime.

set -euo pipefail

ORT_WEB_VERSION="1.24.3"
BASE_URL="https://cdn.pyke.io/0/pyke:ort-rs/web@${ORT_WEB_VERSION}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${1:-${REPO_ROOT}/crates/nemotron-asr/vendor/ort-web-${ORT_WEB_VERSION}}"

ASSETS=(
    "ort.wasm.min.js"
    "ort-wasm-simd-threaded.wasm"
    "ort-wasm-simd-threaded.mjs"
)

# SHA-256 hashes from B3 spike verification (2026-06-04).
# Stored as parallel arrays instead of an associative array with dotted keys
# to avoid zsh warnings on keys containing '.' characters.
HASH_KEYS=(
    "ort.wasm.min.js"
    "ort-wasm-simd-threaded.wasm"
    "ort-wasm-simd-threaded.mjs"
)
HASH_VALS=(
    "4043d2deda6a2e2fc783afc2b06d984068808181b88d451862c1230c433fce7a"
    "be0e129949062ad50290ef94683fac8be5bb6156f709e030b7a5f1661a2f6c17"
    "5687566b1bc1c8cf628d76c2ddb16b2a3b81a7997273d4666564880495088e57"
)

# Lookup function: get_hash <asset-name>
get_hash() {
    local name="$1"
    local i
    for i in "${!HASH_KEYS[@]}"; do
        if [[ "${HASH_KEYS[$i]}" == "$name" ]]; then
            echo "${HASH_VALS[$i]}"
            return 0
        fi
    done
    echo ""
    return 1
}

mkdir -p "$OUTPUT_DIR"

echo "==> Downloading ort-web ${ORT_WEB_VERSION} assets to ${OUTPUT_DIR}/"
for asset in "${ASSETS[@]}"; do
    dest="${OUTPUT_DIR}/${asset}"
    if [[ -f "$dest" ]]; then
        echo "  SKIP: ${asset} (already exists)"
    else
        echo "  GET:  ${BASE_URL}/${asset}"
        curl -fSL --max-time 120 "${BASE_URL}/${asset}" -o "$dest"
    fi
done

echo ""
echo "==> Verifying SHA-256 hashes"
ALL_OK=1
for asset in "${ASSETS[@]}"; do
    dest="${OUTPUT_DIR}/${asset}"
    expected=$(get_hash "$asset")
    if command -v sha256sum &>/dev/null; then
        actual=$(sha256sum "$dest" | awk '{print $1}')
    else
        # macOS
        actual=$(shasum -a 256 "$dest" | awk '{print $1}')
    fi
    if [[ "$actual" == "$expected" ]]; then
        echo "  OK:   ${asset}"
    else
        echo "  FAIL: ${asset}"
        echo "        expected: ${expected}"
        echo "        got:      ${actual}"
        ALL_OK=0
    fi
done

if [[ "$ALL_OK" -eq 1 ]]; then
    echo ""
    echo "All assets verified. Manifest:"
    echo "  Source:       ${BASE_URL}/"
    echo "  ort-web:      ${ORT_WEB_VERSION} (onnxruntime-web 1.24)"
    echo "  Build:        FEATURE_NONE (CPU-only, no WebGPU/WebGL)"
    echo ""
    for asset in "${ASSETS[@]}"; do
        dest="${OUTPUT_DIR}/${asset}"
        size=$(ls -lh "$dest" | awk '{print $5}')
        hash=$(get_hash "$asset")
        echo "  ${asset}  ${size}  sha256:${hash:0:16}..."
    done
else
    echo ""
    echo "ERROR: Hash mismatch — do not use these assets." >&2
    exit 1
fi
