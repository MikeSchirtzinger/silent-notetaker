#!/usr/bin/env bash
# scripts/browser-wasm-tests.sh — In-repo replacement for the broken wasm-pack
# spike runner that hit chromedriver/Chrome major-version mismatch.
#
# Problem (d3 notes): wasm-pack 0.13.1 cached chromedriver 149 against Chrome 148,
# causing WebDriver session failures. The root cause: wasm-pack downloads the
# "latest-known-good" chromedriver, which may lag behind or skip minor builds of
# the installed Chrome.
#
# Solution: pin chromedriver to the installed Chrome's MAJOR.MINOR.BUILD, using
# the chrome-for-testing known-good-versions JSON API to find the highest
# available patch in the same build series.
#
#   1. Detect installed Chrome version (major.minor.build).
#   2. Query chrome-for-testing API for the highest matching patch.
#   3. Download and cache chromedriver to
#      <repo_root>/vendor/chromedriver-<full-version>-<platform>/.
#   4. Pass --chromedriver <path> to wasm-pack (supported in 0.13.1).
#      NOTE: wasm-pack 0.13.1 supports --chromedriver via CLI flag.
#      The CHROMEDRIVER env var is respected by wasm-bindgen-test-runner
#      (the underlying runner), but wasm-pack 0.13.1 does not forward it —
#      use --chromedriver instead.
#   5. Run the browser_smoke test suite from crates/nemotron-asr.
#
# Usage (from repo root):
#   ./scripts/browser-wasm-tests.sh [--no-vendor-server]
#
#   --no-vendor-server: skip starting the local ort-web vendor HTTP server
#                       (useful when the server is already running on port 19999).
#
# Expected output (4 tests):
#   test mel_filterbank_shape ... ok
#   test mel_filterbank_values ... ok
#   test mel_basic ... ok
#   test browser_smoke ... ok
#   test result: ok. 4 passed; 0 failed; 0 ignored
#
# CI mirroring: .github/workflows/ci.yml browser-wasm job uses the same
# chromedriver pinning logic (fetched fresh per run on ubuntu-latest because the
# chrome-for-testing CDN always has the matching ubuntu chrome version).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE_DIR="${REPO_ROOT}/crates/nemotron-asr"
ORT_WEB_VERSION="1.24.3"
VENDOR_DIR="${REPO_ROOT}/crates/nemotron-asr/vendor/ort-web-${ORT_WEB_VERSION}"
CHROMEDRIVER_CACHE_DIR="${REPO_ROOT}/vendor/chromedriver"

START_VENDOR_SERVER=1
for arg in "$@"; do
    case "$arg" in
        --no-vendor-server) START_VENDOR_SERVER=0 ;;
        *) echo "Unknown argument: $arg" >&2; exit 1 ;;
    esac
done

echo ""
echo "==> browser-wasm-tests.sh"
echo "    crate: ${CRATE_DIR}"

# ---------------------------------------------------------------------------
# Step 1: Detect Chrome version
# ---------------------------------------------------------------------------
echo ""
echo "==> Detecting installed Chrome version..."
CHROME_BIN=""
if [[ "$OSTYPE" == "darwin"* ]]; then
    CHROME_BIN="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
elif command -v google-chrome &>/dev/null; then
    CHROME_BIN="google-chrome"
elif command -v chromium-browser &>/dev/null; then
    CHROME_BIN="chromium-browser"
elif command -v chromium &>/dev/null; then
    CHROME_BIN="chromium"
else
    echo "ERROR: Chrome not found. Install Google Chrome." >&2
    exit 1
fi

CHROME_VERSION_RAW=$("$CHROME_BIN" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [[ -z "$CHROME_VERSION_RAW" ]]; then
    echo "ERROR: Could not parse Chrome version from: $("$CHROME_BIN" --version 2>/dev/null)" >&2
    exit 1
fi
echo "    Chrome: ${CHROME_VERSION_RAW}"

# Parse major.minor.build (drop the 4th patch component — we'll find the best
# matching chromedriver patch from the known-good-versions API).
CHROME_MAJOR=$(echo "$CHROME_VERSION_RAW" | cut -d. -f1)
CHROME_MINOR=$(echo "$CHROME_VERSION_RAW" | cut -d. -f2)
CHROME_BUILD=$(echo "$CHROME_VERSION_RAW" | cut -d. -f3)
CHROME_PATCH=$(echo "$CHROME_VERSION_RAW" | cut -d. -f4)
CHROME_PREFIX="${CHROME_MAJOR}.${CHROME_MINOR}.${CHROME_BUILD}"
echo "    Build prefix to match: ${CHROME_PREFIX}.x"

# ---------------------------------------------------------------------------
# Step 2: Find best matching chromedriver via chrome-for-testing API
# ---------------------------------------------------------------------------
echo ""
echo "==> Querying chrome-for-testing for chromedriver ${CHROME_PREFIX}.x..."
KNOWN_GOOD_URL="https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json"

# Determine platform string
if [[ "$OSTYPE" == "darwin"* ]]; then
    ARCH=$(uname -m)
    if [[ "$ARCH" == "arm64" ]]; then
        PLATFORM="mac-arm64"
    else
        PLATFORM="mac-x64"
    fi
else
    PLATFORM="linux64"
fi
echo "    Platform: ${PLATFORM}"

# Download the known-good JSON and find the best matching version.
# Strategy: among all versions with the same major.minor.build prefix, pick
# the one with the highest patch number (closest to the installed Chrome).
MATCHED_VERSION=$(python3 - <<PYEOF
import json, urllib.request, sys

url = "${KNOWN_GOOD_URL}"
try:
    with urllib.request.urlopen(url, timeout=30) as r:
        data = json.load(r)
except Exception as e:
    print(f"ERROR: {e}", file=sys.stderr)
    sys.exit(1)

prefix = "${CHROME_PREFIX}."
platform = "${PLATFORM}"

candidates = []
for v in data.get("versions", []):
    ver = v.get("version", "")
    if not ver.startswith(prefix):
        continue
    # Check chromedriver is available for this platform
    dls = v.get("downloads", {}).get("chromedriver", [])
    for dl in dls:
        if dl.get("platform") == platform:
            patch = int(ver.split(".")[-1])
            candidates.append((patch, ver, dl["url"]))
            break

if not candidates:
    print(f"ERROR: no chromedriver found for prefix {prefix} platform {platform}", file=sys.stderr)
    sys.exit(1)

# Pick highest patch
candidates.sort(reverse=True)
_, best_ver, best_url = candidates[0]
print(f"{best_ver}:{best_url}")
PYEOF
)

if [[ -z "$MATCHED_VERSION" ]]; then
    echo "ERROR: Could not find matching chromedriver version." >&2
    exit 1
fi

CHROMEDRIVER_VERSION=$(echo "$MATCHED_VERSION" | cut -d: -f1)
CHROMEDRIVER_URL=$(echo "$MATCHED_VERSION" | cut -d: -f2-)
echo "    Best match: chromedriver ${CHROMEDRIVER_VERSION}"

# ---------------------------------------------------------------------------
# Step 3: Download and cache chromedriver
# ---------------------------------------------------------------------------
CACHE_PATH="${CHROMEDRIVER_CACHE_DIR}/${CHROMEDRIVER_VERSION}-${PLATFORM}"
CHROMEDRIVER_BIN="${CACHE_PATH}/chromedriver"

if [[ -f "$CHROMEDRIVER_BIN" ]]; then
    echo ""
    echo "==> Chromedriver already cached: ${CHROMEDRIVER_BIN}"
else
    echo ""
    echo "==> Downloading chromedriver ${CHROMEDRIVER_VERSION} for ${PLATFORM}..."
    mkdir -p "$CACHE_PATH"
    TMP_ZIP="/tmp/chromedriver-${CHROMEDRIVER_VERSION}-${PLATFORM}.zip"
    curl -fSL --max-time 120 "$CHROMEDRIVER_URL" -o "$TMP_ZIP"
    echo "    Extracting..."
    # The zip contains a directory like chromedriver-<platform>/chromedriver
    unzip -o -q "$TMP_ZIP" -d "$CACHE_PATH"
    rm -f "$TMP_ZIP"
    # Find the chromedriver binary (may be in a subdirectory)
    FOUND_BIN=$(find "$CACHE_PATH" -name "chromedriver" -type f | head -1)
    if [[ -z "$FOUND_BIN" ]]; then
        echo "ERROR: chromedriver binary not found after extraction." >&2
        exit 1
    fi
    if [[ "$FOUND_BIN" != "$CHROMEDRIVER_BIN" ]]; then
        mv "$FOUND_BIN" "$CHROMEDRIVER_BIN"
    fi
    chmod +x "$CHROMEDRIVER_BIN"
    echo "    Cached: ${CHROMEDRIVER_BIN}"
fi

echo "    Version: $("$CHROMEDRIVER_BIN" --version 2>/dev/null || echo 'unknown')"

# ---------------------------------------------------------------------------
# Step 4: Vendor ort-web assets (if not already present)
# ---------------------------------------------------------------------------
echo ""
if [[ ! -f "${VENDOR_DIR}/ort.wasm.min.js" ]]; then
    echo "==> Vendoring ort-web ${ORT_WEB_VERSION} assets..."
    "${REPO_ROOT}/scripts/vendor-ort-web.sh"
else
    echo "==> ort-web assets already vendored (${VENDOR_DIR})."
fi

# ---------------------------------------------------------------------------
# Step 5: Start local ort-web vendor server
# ---------------------------------------------------------------------------
VENDOR_PID=""
if [[ "$START_VENDOR_SERVER" -eq 1 ]]; then
    echo ""
    echo "==> Starting local ort-web vendor server (port 19999)..."
    python3 -m http.server 19999 --directory "$VENDOR_DIR" &
    VENDOR_PID=$!
    # Wait for server to be ready
    for i in $(seq 1 10); do
        if curl -sf http://localhost:19999/ort.wasm.min.js -o /dev/null 2>/dev/null; then
            echo "    Server ready (pid ${VENDOR_PID})"
            break
        fi
        sleep 1
    done
fi

cleanup() {
    if [[ -n "${VENDOR_PID}" ]]; then
        kill "${VENDOR_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Step 6: Write webdriver.json (CDN block + headless Chrome config)
# ---------------------------------------------------------------------------
echo ""
echo "==> Writing webdriver.json (CDN block, headless Chrome)..."
pushd "${CRATE_DIR}" > /dev/null

# Preserve existing webdriver.json if any (restore on exit)
ORIG_WEBDRIVER_JSON=""
if [[ -f webdriver.json ]]; then
    ORIG_WEBDRIVER_JSON=$(cat webdriver.json)
fi

restore_webdriver_json() {
    popd > /dev/null 2>&1 || true
    if [[ -n "$ORIG_WEBDRIVER_JSON" ]]; then
        echo "$ORIG_WEBDRIVER_JSON" > "${CRATE_DIR}/webdriver.json"
    fi
    cleanup
}
trap restore_webdriver_json EXIT

cat > webdriver.json << 'WEBDRIVER_EOF'
{
  "goog:chromeOptions": {
    "args": [
      "--disable-dev-shm-usage",
      "--no-sandbox",
      "--disable-gpu",
      "--headless=new",
      "--enable-logging",
      "--log-level=0",
      "--host-resolver-rules=MAP cdn.pyke.io 127.0.0.2,MAP signal.pyke.io 127.0.0.2"
    ]
  }
}
WEBDRIVER_EOF

# ---------------------------------------------------------------------------
# Step 7: Run browser_smoke tests via wasm-pack
# ---------------------------------------------------------------------------
echo ""
echo "==> Running browser_smoke tests..."
echo "    wasm-pack test --headless --chrome --chromedriver ${CHROMEDRIVER_BIN}"
echo "    test: crates/nemotron-asr/tests/browser_smoke.rs"
echo ""

# wasm-pack 0.13.1 supports --chromedriver flag.
# The CHROMEDRIVER env var is respected by wasm-bindgen-test-runner directly,
# but wasm-pack 0.13.1 does NOT forward it to the runner — pass it as CLI flag.
wasm-pack test --headless --chrome \
    --chromedriver "${CHROMEDRIVER_BIN}" \
    -- --test browser_smoke

EXIT_CODE=$?
# Restore webdriver.json and clean up (mirrors what the EXIT trap does).
trap - EXIT
if [[ -n "$ORIG_WEBDRIVER_JSON" ]]; then
    echo "$ORIG_WEBDRIVER_JSON" > "${CRATE_DIR}/webdriver.json"
fi
popd > /dev/null 2>&1 || true
cleanup

if [[ "$EXIT_CODE" -eq 0 ]]; then
    echo ""
    echo "==> browser_smoke PASSED"
else
    echo ""
    echo "==> browser_smoke FAILED (exit code: ${EXIT_CODE})"
    exit "$EXIT_CODE"
fi
