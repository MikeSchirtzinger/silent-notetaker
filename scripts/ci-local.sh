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
#   9. cargo run -p xtask -- model-audit          [hard failure — E1 removed titanet.onnx + mel_fb.json]
#  10. cargo run -p xtask -- gen-headers --check  [hard failure — E1 regenerated _headers]
#  11. lychee docs/ README.md                     [link-check, --skip-link-check to skip; non-blocking]
#
# Expected outcome (post-E1):
#   Gates 1–10: GREEN
#   Gate 11 (link-check): non-blocking warning

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
    echo -e "${BOLD}==> browser-wasm: scripts/browser-wasm-tests.sh (pinned chromedriver)${RESET}"
    # Delegate to the in-repo script which pins chromedriver to the installed
    # Chrome's major.minor.build — prevents the driver/browser version mismatch
    # that broke the original spike runner (chromedriver 149 vs Chrome 148).
    WASM_EXIT=0
    "${REPO_ROOT}/scripts/browser-wasm-tests.sh" 2>&1 || WASM_EXIT=$?

    if [[ "$WASM_EXIT" -eq 0 ]]; then
        results+=("  ${PASS}             browser-wasm: wasm-pack headless Chrome (browser_smoke, pinned chromedriver)")
    else
        results+=("  ${FAIL}             browser-wasm: wasm-pack headless Chrome (browser_smoke, pinned chromedriver)")
        overall_ok=0
    fi
else
    echo ""
    echo -e "${YELLOW}  SKIP  browser-wasm: wasm-pack not installed (run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh)${RESET}"
    results+=("  ${SKIP}            browser-wasm: wasm-pack not installed")
fi

# ---------------------------------------------------------------------------
# Gate 9: xtask model-audit (hard failure — E1 removed titanet.onnx + mel_fb.json)
# ---------------------------------------------------------------------------
run_gate "xtask-gates: model-audit" false \
    cargo run -p xtask -- model-audit

# ---------------------------------------------------------------------------
# Gate 10: xtask gen-headers --check (hard failure — E1 regenerated _headers)
# ---------------------------------------------------------------------------
run_gate "xtask-gates: gen-headers --check" false \
    cargo run -p xtask -- gen-headers --check --out _headers

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
