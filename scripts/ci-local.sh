#!/usr/bin/env bash
# scripts/ci-local.sh — Run the same CI gates locally in order.
#
# Usage (from repo root):
#   ./scripts/ci-local.sh [--skip-wasm] [--skip-link-check]
#
# Gates (in order):
#   1. cargo fmt --all --check
#   2. cargo check --workspace --all-targets
#   3. cargo test --workspace --all-targets
#   4. cargo clippy --workspace --all-targets -- -D warnings
#   5. cargo deny check                           [supply-chain]
#   6. cargo audit                                [supply-chain]
#   7. cargo test -p silent-core export_bindings + git diff --exit-code  [boundary-fresh]
#   8. wasm-pack test --headless --chrome (browser_smoke)                [browser-wasm, --skip-wasm to skip]
#   9. cargo run -p xtask -- model-audit          [EXPECTED FAIL until E1 — continue-on-error]
#  10. cargo run -p xtask -- gen-headers --check  [EXPECTED FAIL until D2 — continue-on-error]
#  11. lychee docs/ README.md                     [link-check, --skip-link-check to skip; non-blocking]
#
# Expected outcome:
#   Gates 1–8: GREEN
#   Gate 9 (model-audit): RED (expected — titanet.onnx + mel_fb.json committed until E1)
#   Gate 10 (gen-headers): RED (expected — d2-xtask subcommand pending)
#   Gate 11 (link-check): non-blocking warning
#
# E1 handoff: when E1 removes titanet.onnx + mel_fb.json and D2 ships gen-headers,
#   flip the two "continue-on-error" gates to hard failures and update this comment.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SKIP_WASM=0
SKIP_LINK_CHECK=0
for arg in "$@"; do
    case "$arg" in
        --skip-wasm) SKIP_WASM=1 ;;
        --skip-link-check) SKIP_LINK_CHECK=1 ;;
        *) echo "Unknown argument: $arg" >&2; exit 1 ;;
    esac
done

# Colour helpers (disabled in CI / non-TTY environments)
if [[ -t 1 ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' BOLD='' RESET=''
fi

PASS="${GREEN}PASS${RESET}"
FAIL="${RED}FAIL${RESET}"
EXPECTED_FAIL="${YELLOW}EXPECTED-FAIL${RESET}"
SKIP="${YELLOW}SKIP${RESET}"

results=()
overall_ok=1

run_gate() {
    local label="$1"
    local expected_fail="${2:-false}"   # "true" = annotate as expected failure
    shift 2
    echo ""
    echo -e "${BOLD}==> ${label}${RESET}"
    echo "    cmd: $*"
    if "$@" 2>&1; then
        if [[ "$expected_fail" == "true" ]]; then
            results+=("  ${YELLOW}UNEXPECTED-PASS${RESET}  ${label}  (expected to fail — check E1/D2 status)")
        else
            results+=("  ${PASS}             ${label}")
        fi
    else
        if [[ "$expected_fail" == "true" ]]; then
            results+=("  ${EXPECTED_FAIL}    ${label}  (continue-on-error — see E1/D2 notes)")
        else
            results+=("  ${FAIL}             ${label}")
            overall_ok=0
        fi
    fi
}

echo ""
echo -e "${BOLD}Silent Notetaker — CI local run${RESET}"
echo "Branch: $(git branch --show-current)"
echo "Commit: $(git rev-parse --short HEAD)"
echo ""

# ---------------------------------------------------------------------------
# Gate 1: Format
# ---------------------------------------------------------------------------
run_gate "fmt" false \
    cargo fmt --all --check

# ---------------------------------------------------------------------------
# Gate 2: Check
# ---------------------------------------------------------------------------
run_gate "check" false \
    cargo check --workspace --all-targets

# ---------------------------------------------------------------------------
# Gate 3: Test
# ---------------------------------------------------------------------------
run_gate "test" false \
    cargo test --workspace --all-targets

# ---------------------------------------------------------------------------
# Gate 4: Clippy
# ---------------------------------------------------------------------------
run_gate "clippy" false \
    cargo clippy --workspace --all-targets -- -D warnings

# ---------------------------------------------------------------------------
# Gate 5: cargo-deny
# ---------------------------------------------------------------------------
if command -v cargo-deny &>/dev/null || cargo deny --version &>/dev/null 2>&1; then
    run_gate "supply-chain: cargo deny check" false \
        cargo deny check
else
    echo -e "${YELLOW}  SKIP  supply-chain: cargo deny check (cargo-deny not installed; run: cargo install cargo-deny)${RESET}"
    results+=("  ${SKIP}            supply-chain: cargo deny check (not installed)")
fi

# ---------------------------------------------------------------------------
# Gate 6: cargo-audit
# ---------------------------------------------------------------------------
if command -v cargo-audit &>/dev/null || cargo audit --version &>/dev/null 2>&1; then
    run_gate "supply-chain: cargo audit" false \
        cargo audit
else
    echo -e "${YELLOW}  SKIP  supply-chain: cargo audit (cargo-audit not installed; run: cargo install cargo-audit)${RESET}"
    results+=("  ${SKIP}            supply-chain: cargo audit (not installed)")
fi

# ---------------------------------------------------------------------------
# Gate 7: Boundary freshness
# ---------------------------------------------------------------------------
echo ""
echo -e "${BOLD}==> boundary-fresh: export_bindings + git diff${RESET}"
if cargo test -p silent-core export_bindings 2>&1; then
    if git diff --exit-code crates/silent-core/bindings/ 2>&1; then
        results+=("  ${PASS}             boundary-fresh: bindings are up to date")
    else
        echo -e "${RED}  ERROR: Stale bindings detected — run 'cargo test -p silent-core export_bindings' and commit.${RESET}"
        results+=("  ${FAIL}             boundary-fresh: stale bindings (commit the updated bindings/)")
        overall_ok=0
    fi
else
    results+=("  ${FAIL}             boundary-fresh: export_bindings test failed")
    overall_ok=0
fi

# ---------------------------------------------------------------------------
# Gate 8: Browser WASM (wasm-pack headless Chrome)
# ---------------------------------------------------------------------------
if [[ "$SKIP_WASM" -eq 1 ]]; then
    echo ""
    echo -e "${YELLOW}  SKIP  browser-wasm: wasm-pack test (--skip-wasm passed)${RESET}"
    results+=("  ${SKIP}            browser-wasm: skipped (--skip-wasm)")
elif command -v wasm-pack &>/dev/null; then
    echo ""
    echo -e "${BOLD}==> browser-wasm: vendor ort-web assets + wasm-pack headless Chrome${RESET}"

    # Vendor assets if missing
    ORT_WEB_VERSION="1.24.3"
    VENDOR_DIR="${REPO_ROOT}/crates/nemotron-asr/vendor/ort-web-${ORT_WEB_VERSION}"
    if [[ ! -f "${VENDOR_DIR}/ort.wasm.min.js" ]]; then
        echo "  Vendoring ort-web assets..."
        "${REPO_ROOT}/scripts/vendor-ort-web.sh"
    else
        echo "  ort-web assets already vendored."
    fi

    # Start local vendor server
    python3 -m http.server 19999 --directory "$VENDOR_DIR" &
    VENDOR_PID=$!
    for i in $(seq 1 10); do
        curl -sf http://localhost:19999/ort.wasm.min.js -o /dev/null 2>/dev/null && break
        sleep 1
    done
    echo "  Vendor server running (pid ${VENDOR_PID})."

    # Ensure webdriver.json blocks CDN (copy B3's config)
    pushd "${REPO_ROOT}/crates/nemotron-asr" > /dev/null
    ORIG_WEBDRIVER_JSON=""
    if [[ -f webdriver.json ]]; then
        ORIG_WEBDRIVER_JSON=$(cat webdriver.json)
    fi
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

    WASM_EXIT=0
    wasm-pack test --headless --chrome -- --test browser_smoke 2>&1 || WASM_EXIT=$?

    # Restore original webdriver.json
    if [[ -n "$ORIG_WEBDRIVER_JSON" ]]; then
        echo "$ORIG_WEBDRIVER_JSON" > webdriver.json
    fi

    kill "$VENDOR_PID" 2>/dev/null || true
    popd > /dev/null

    if [[ "$WASM_EXIT" -eq 0 ]]; then
        results+=("  ${PASS}             browser-wasm: wasm-pack headless Chrome (browser_smoke)")
    else
        results+=("  ${FAIL}             browser-wasm: wasm-pack headless Chrome (browser_smoke)")
        overall_ok=0
    fi
else
    echo ""
    echo -e "${YELLOW}  SKIP  browser-wasm: wasm-pack not installed (run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh)${RESET}"
    results+=("  ${SKIP}            browser-wasm: wasm-pack not installed")
fi

# ---------------------------------------------------------------------------
# Gate 9: xtask model-audit (EXPECTED FAIL until E1)
# ---------------------------------------------------------------------------
echo ""
echo -e "${BOLD}==> xtask-gates: model-audit [EXPECTED FAIL until E1]${RESET}"
echo "    NOTE: titanet.onnx + mel_fb.json are committed until E1 removes them."
echo "    This step uses continue-on-error — failure is annotated but does not block."
echo "    E1: flip this to a hard failure when titanet.onnx/mel_fb.json are removed."
MODEL_AUDIT_EXIT=0
cargo run -p xtask -- model-audit 2>&1 || MODEL_AUDIT_EXIT=$?
if [[ "$MODEL_AUDIT_EXIT" -ne 0 ]]; then
    echo -e "${YELLOW}  EXPECTED-FAIL: model-audit failed (titanet.onnx + mel_fb.json at root).${RESET}"
    echo -e "${YELLOW}  Tracked to Task E1. Flip continue-on-error: false after E1 ships.${RESET}"
    results+=("  ${EXPECTED_FAIL}    xtask-gates: model-audit (titanet.onnx/mel_fb.json — E1 removes these)")
else
    echo -e "${GREEN}  model-audit PASSED (weights may have been removed — verify E1 complete).${RESET}"
    results+=("  ${PASS}  [E1 may be complete?]  xtask-gates: model-audit")
fi

# ---------------------------------------------------------------------------
# Gate 10: xtask gen-headers --check (EXPECTED FAIL until D2 ships)
# ---------------------------------------------------------------------------
echo ""
echo -e "${BOLD}==> xtask-gates: gen-headers --check [EXPECTED FAIL until D2/E1]${RESET}"
echo "    NOTE: d2-xtask is building this subcommand concurrently."
echo "    E1: flip this to a hard failure once D2 ships and _headers parity is verified."
GEN_HEADERS_EXIT=0
cargo run -p xtask -- gen-headers --check --out _headers 2>&1 || GEN_HEADERS_EXIT=$?
if [[ "$GEN_HEADERS_EXIT" -ne 0 ]]; then
    echo -e "${YELLOW}  EXPECTED-FAIL: gen-headers --check failed (D2 in progress).${RESET}"
    results+=("  ${EXPECTED_FAIL}    xtask-gates: gen-headers --check (D2 in progress — continue-on-error)")
else
    results+=("  ${PASS}  [D2 may be complete?]  xtask-gates: gen-headers --check")
fi

# ---------------------------------------------------------------------------
# Gate 11: Link check (non-blocking)
# ---------------------------------------------------------------------------
if [[ "$SKIP_LINK_CHECK" -eq 1 ]]; then
    echo ""
    echo -e "${YELLOW}  SKIP  link-check (--skip-link-check passed)${RESET}"
    results+=("  ${SKIP}            link-check: skipped (--skip-link-check)")
elif command -v lychee &>/dev/null; then
    echo ""
    echo -e "${BOLD}==> link-check: lychee docs/ README.md (non-blocking)${RESET}"
    LYCHEE_EXIT=0
    lychee \
        --exclude "localhost" \
        --exclude "cdn-lfs\\.huggingface\\.co" \
        --exclude "cdn-lfs-us-1\\.huggingface\\.co" \
        --exclude "huggingface\\.co/resolve" \
        --timeout 30 \
        --max-retries 3 \
        docs/ README.md 2>&1 || LYCHEE_EXIT=$?
    if [[ "$LYCHEE_EXIT" -ne 0 ]]; then
        echo -e "${YELLOW}  WARNING: lychee found broken/unreachable links (non-blocking — see output above).${RESET}"
        results+=("  ${YELLOW}WARN${RESET}             link-check: lychee found issues (non-blocking)")
    else
        results+=("  ${PASS}             link-check: lychee (no broken links)")
    fi
else
    echo ""
    echo -e "${YELLOW}  SKIP  link-check: lychee not installed (run: cargo install lychee)${RESET}"
    results+=("  ${SKIP}            link-check: lychee not installed")
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo -e "${BOLD}======================== CI Local Results ========================${RESET}"
for r in "${results[@]}"; do
    echo -e "$r"
done
echo -e "${BOLD}=================================================================${RESET}"
echo ""

if [[ "$overall_ok" -eq 1 ]]; then
    echo -e "${GREEN}${BOLD}All hard gates passed.${RESET}"
    echo "Expected failures (model-audit, gen-headers) are annotated above — see E1/D2."
    exit 0
else
    echo -e "${RED}${BOLD}One or more hard gates FAILED. See output above.${RESET}"
    exit 1
fi
