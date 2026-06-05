#!/usr/bin/env bash
# scripts/build-wasm.sh — Reproducible wasm build for Silent Notetaker (PRD R8 / Task K1).
#
# Builds the two deployed wasm crates with wasm-pack and emits a sha256 manifest.
# The builds are byte-for-byte reproducible across machines and clone paths: the
# embedded source paths (the std library, the crates.io registry, and the
# workspace root) are remapped to stable tokens via `--remap-path-prefix`, so the
# bytes do not depend on WHERE the toolchain, cargo cache, or checkout live.
#
#   crates/silent-web    → crates/silent-web/pkg/silent_web_bg.wasm
#   crates/nemotron-asr  → crates/nemotron-asr/pkg/nemotron_asr_bg.wasm
#
# Why this exists (R8): `wasm-pack` ended the no-build-step era. The honest
# replacement for "audit the single file" is "verify the binary you are running":
# the build must be reproducible so the hash the app displays (settings → About)
# can be matched against a fresh local build. This script is that build.
#
# Usage (from repo root or anywhere):
#   ./scripts/build-wasm.sh              # build both crates + write the manifest
#   ./scripts/build-wasm.sh --manifest-only PATH   # (re)compute the manifest from existing pkg/
#
# Output:
#   - crates/{silent-web,nemotron-asr}/pkg/                (wasm-pack output)
#   - <stdout>                                            the sha256 lines
#
# Reproducibility pins (must match for byte-identical output — see README):
#   - Rust toolchain : pinned by rust-toolchain.toml (channel 1.95.0)
#   - Cargo.lock     : committed (exact dependency graph)
#   - wasm-pack      : 0.13.1   (bundles wasm-bindgen 0.2.100 + wasm-opt v117)
#   - profile.release: opt-level "s" + lto = true (root Cargo.toml)
#
# A mismatching wasm-pack version is the most likely cause of a hash mismatch on
# another machine: wasm-bindgen and wasm-opt are vendored by wasm-pack and their
# output bytes are version-specific. The script prints the detected version so a
# mismatch is visible, not silent.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# The two deployed wasm artifacts, in deploy order. Each entry is
# "<crate-dir>|<wasm-file-relative-to-repo-root>".
ARTIFACTS=(
  "crates/silent-web|crates/silent-web/pkg/silent_web_bg.wasm"
  "crates/nemotron-asr|crates/nemotron-asr/pkg/nemotron_asr_bg.wasm"
)

# sha256 helper that works on both macOS (shasum) and Linux (sha256sum).
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# --manifest-only: recompute the manifest from already-built pkg/ dirs without
# rebuilding (used by the deploy gate / CI to hash what is about to ship).
if [[ "${1:-}" == "--manifest-only" ]]; then
  for entry in "${ARTIFACTS[@]}"; do
    wasm="${entry##*|}"
    [[ -f "$wasm" ]] || { echo "error: missing $wasm (build first)" >&2; exit 1; }
    printf '%s  %s\n' "$(sha256_of "$wasm")" "$wasm"
  done
  exit 0
fi

# ---------------------------------------------------------------------------
# Build the deterministic-path RUSTFLAGS.
#
# Three absolute prefixes leak into the wasm as panic/location strings via the
# `file!()` macro and must be remapped to stable tokens:
#   1. the crates.io registry source     ($CARGO_HOME/registry)  → /cargo-home
#   2. the rustup toolchain sysroot       (rustc --print sysroot) → /rust-toolchain
#   3. the workspace root                 ($REPO_ROOT)            → /silent-notetaker
#
# Both the symlinked and the symlink-resolved (`pwd -P`) form of CARGO_HOME and
# the sysroot are remapped, because cargo emits whichever the filesystem hands
# it. `trim-paths` would be the cleaner fix but is nightly-only as of cargo
# 1.95; `--remap-path-prefix` is the stable-toolchain equivalent.
# ---------------------------------------------------------------------------
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
SYSROOT="$(rustc --print sysroot)"
CARGO_HOME_REAL="$(cd "$CARGO_HOME_DIR" 2>/dev/null && pwd -P || echo "$CARGO_HOME_DIR")"
SYSROOT_REAL="$(cd "$SYSROOT" 2>/dev/null && pwd -P || echo "$SYSROOT")"
REPO_ROOT_REAL="$(pwd -P)"

REMAP=(
  "--remap-path-prefix=${CARGO_HOME_REAL}=/cargo-home"
  "--remap-path-prefix=${CARGO_HOME_DIR}=/cargo-home"
  "--remap-path-prefix=${SYSROOT_REAL}=/rust-toolchain"
  "--remap-path-prefix=${SYSROOT}=/rust-toolchain"
  "--remap-path-prefix=${REPO_ROOT_REAL}=/silent-notetaker"
  "--remap-path-prefix=${REPO_ROOT}=/silent-notetaker"
)

# Print the reproducibility-relevant versions so a mismatch is loud, not silent.
echo "▸ Reproducible wasm build (PRD R8)"
echo "    rustc      : $(rustc --version)"
echo "    wasm-pack  : $(wasm-pack --version 2>/dev/null || echo 'NOT FOUND — install wasm-pack 0.13.1')"
echo "    sysroot    : ${SYSROOT}  → /rust-toolchain"
echo "    cargo home : ${CARGO_HOME_DIR}  → /cargo-home"
echo "    workspace  : ${REPO_ROOT}  → /silent-notetaker"
echo ""

# CARGO_BUILD_RUSTFLAGS applies to every rustc invocation cargo makes, including
# transitive dependency crates — exactly what we need for the std/registry paths.
export CARGO_BUILD_RUSTFLAGS="${REMAP[*]}"

for entry in "${ARTIFACTS[@]}"; do
  crate_dir="${entry%%|*}"
  echo "▸ wasm-pack build ${crate_dir} (--target web --release, remapped paths) ..."
  wasm-pack build "${crate_dir}" --target web --release
  echo ""
done

# ---------------------------------------------------------------------------
# Emit the sha256 manifest. It is written to <repo-root>/wasm-hashes.txt (so the
# root-served app from start.sh can verify itself in-app — Settings → About) AND
# printed to stdout (callers like deploy-cloudflare.sh / CI capture it). The
# manifest paths are relative to the app root, matching what the in-app verifier
# fetches. The root copy is gitignored — it is a build artifact, never committed.
# ---------------------------------------------------------------------------
ROOT_MANIFEST="${REPO_ROOT}/wasm-hashes.txt"
{
  echo "# Silent Notetaker — local wasm sha256 manifest (PRD R8)."
  echo "# Written by scripts/build-wasm.sh. Reproduce: ./scripts/build-wasm.sh."
  echo "# Format: <sha256>  <path-relative-to-app-root>"
} > "$ROOT_MANIFEST"

echo "▸ sha256 of deployed wasm artifacts (also written to wasm-hashes.txt):"
for entry in "${ARTIFACTS[@]}"; do
  wasm="${entry##*|}"
  line="$(sha256_of "$wasm")  ${wasm}"
  echo "$line" | tee -a "$ROOT_MANIFEST"
done
