#!/bin/bash
# ┌──────────────────────────────────────────┐
# │  Silent Notetaker — One-Click Launcher   │
# │  Starts HTTP server + Claude Bridge      │
# └──────────────────────────────────────────┘

set -e
cd "$(dirname "$0")"

# Source shell profile for env vars (non-login shells miss these)
[ -f "$HOME/.zshrc" ] && source "$HOME/.zshrc" 2>/dev/null || true
[ -f "$HOME/.bash_profile" ] && source "$HOME/.bash_profile" 2>/dev/null || true

# Colors
GREEN='\033[0;32m'
PURPLE='\033[0;35m'
DIM='\033[2m'
NC='\033[0m'

echo ""
echo -e "${GREEN}Silent Notetaker${NC}"
echo -e "${DIM}────────────────────────────────${NC}"

# Check for uv (needed for bridge)
BRIDGE_AVAILABLE=false
if command -v uv &>/dev/null; then
  BRIDGE_AVAILABLE=true
else
  echo -e "${DIM}⚠  uv not found — Claude Bridge disabled${NC}"
  echo -e "${DIM}   Install: curl -LsSf https://astral.sh/uv/install.sh | sh${NC}"
fi

# Kill any existing instances on our ports
lsof -ti:8080 2>/dev/null | xargs kill 2>/dev/null || true
lsof -ti:8765 2>/dev/null | xargs kill 2>/dev/null || true

# Start HTTP server.
# MUST send cross-origin-isolation headers (COOP/COEP) so the browser exposes
# SharedArrayBuffer → multithreaded WASM (Qwen/onnxruntime-web run ~3-4x faster).
# Prefer the Rust server; fall back to coi-server.py (also sends the headers).
# Never plain `python -m http.server` — no headers → single-threaded WASM.
if command -v cargo &>/dev/null && cargo build --release --quiet --manifest-path server/Cargo.toml 2>/tmp/notetaker-build.log; then
  cargo run --release --quiet --manifest-path server/Cargo.toml -- . 8080 &>/dev/null &
  HTTP_PID=$!
  echo -e "  ${GREEN}✓${NC} HTTP server     → ${GREEN}http://localhost:8080${NC} ${DIM}(Rust · cross-origin isolated)${NC}"
else
  [ -f /tmp/notetaker-build.log ] && echo -e "  ${DIM}⚠  Rust server unavailable — using Python fallback (still isolated)${NC}"
  python3 coi-server.py 8080 &>/dev/null &
  HTTP_PID=$!
  echo -e "  ${GREEN}✓${NC} HTTP server     → ${GREEN}http://localhost:8080${NC} ${DIM}(Python · cross-origin isolated)${NC}"
fi

# Start Claude Bridge
# Auth is handled by bridge.py: Keychain OAuth → env var → saved token → interactive
BRIDGE_PID=""
if [ "$BRIDGE_AVAILABLE" = true ]; then
  uv run bridge.py &
  BRIDGE_PID=$!
  echo -e "  ${PURPLE}✓${NC} Claude Bridge   → ${PURPLE}ws://localhost:8765${NC}"
  echo -e "  ${DIM}  Auth: Keychain OAuth → ANTHROPIC_API_KEY → saved token${NC}"
fi

echo ""

# Open browser (small delay to let servers start / first-run build settle)
sleep 2
if [[ "$OSTYPE" == "darwin"* ]]; then
  open "http://localhost:8080"
elif command -v xdg-open &>/dev/null; then
  xdg-open "http://localhost:8080"
fi

echo -e "${DIM}Press Ctrl+C to stop${NC}"
echo ""

# Cleanup on exit
cleanup() {
  echo ""
  echo -e "${DIM}Shutting down...${NC}"
  kill $HTTP_PID 2>/dev/null || true
  lsof -ti:8080 2>/dev/null | xargs kill 2>/dev/null || true  # cargo run spawns the server as a child
  [ -n "$BRIDGE_PID" ] && kill $BRIDGE_PID 2>/dev/null || true
  echo -e "${GREEN}Done.${NC}"
}
trap cleanup EXIT INT TERM

# Wait
wait
