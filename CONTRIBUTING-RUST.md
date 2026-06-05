# Contributing: Rust Workspace

This document covers the Rust workspace that implements all application logic and
policy for Silent Notetaker. For general contribution guidelines (code style, privacy
conventions, how to propose a change), see `CONTRIBUTING.md`.

---

## Prerequisites

- Rust toolchain matching `rust-toolchain.toml` (the file pins the exact version;
  `rustup` will install it automatically on first `cargo` invocation).
- `wasm-pack` for browser-wasm tests: `cargo install wasm-pack`.
- Chrome or Chromium for browser-wasm tests (see `scripts/browser-wasm-tests.sh`).
- `cargo-deny` and `cargo-audit` for supply-chain gates: `cargo install cargo-deny cargo-audit`.

---

## Gate suite

Every pull request must pass all gates before merging. Run them locally first:

```bash
./scripts/ci-local.sh
```

To skip the browser-wasm tests (requires Chrome):

```bash
./scripts/ci-local.sh --skip-wasm
```

The gates in order:

| # | Command | What it checks |
|---|---|---|
| 1 | `cargo fmt --all --check` | Formatting |
| 2 | `cargo check --workspace --all-targets` | Compilation |
| 3 | `cargo test --workspace --all-targets` | Unit and integration tests |
| 4 | `cargo clippy --workspace --all-targets -- -D warnings` | Lint (no warnings allowed) |
| 5 | `cargo deny check` | Supply-chain (licenses, advisories, duplicates) |
| 6 | `cargo audit` | Known CVEs |
| 7 | `cargo test -p silent-core export_bindings && git diff --exit-code` | ts-rs bindings freshness |
| 8 | `scripts/browser-wasm-tests.sh` | Browser WASM smoke (nemotron-asr) |
| 9 | `cargo run -p xtask -- model-audit` | No committed model weights |
| 10 | `cargo run -p xtask -- gen-headers --check` | `_headers` freshness vs registry |

Gates 1–10 must be green. Gate 11 (link-check via `lychee`) is non-blocking.

**The wasm32 clippy requirement.** Clippy is run for all targets, which includes the
wasm32 target for crates that support it. A warning that only appears on wasm32 is
still a build failure. Use `#[cfg(target_arch = "wasm32")]` to isolate
platform-specific code, and document any `#[allow]` with a rationale at the site.

---

## Rust engineering bar

These are hard requirements, not style suggestions:

- `cargo fmt --all --check` must pass before commit.
- `cargo check --workspace --all-targets` must pass.
- `cargo test --workspace --all-targets` must pass.
- `cargo clippy --workspace --all-targets -- -D warnings` must pass. No warnings.
- No `unwrap()` or `expect()` in production paths. Use `?` propagation or explicit
  `match`. If a panic is genuinely unreachable, document why with a comment and use
  a named constant or a unit test that proves the invariant.
- Every `#[allow(...)]` carries a rationale comment at the site explaining why the
  warning is wrong for this specific case.
- `unsafe_code = "forbid"` is a workspace lint. Any crate that relaxes this must
  document the specific invariant that makes the unsafe block sound.

---

## Golden-test conventions

Golden tests are the primary validation mechanism for Rust policy crates. The
conventions that apply across all crates:

### The loud-skip rule

A golden test that cannot run in the current environment (no GPU, no browser, no
mic) must print a loud skip banner rather than silently passing:

```
==========================================================
SKIP: golden test requires a real browser + microphone
      (NEEDS-BROWSER-TEST — not validated in this environment)
==========================================================
```

A test that silently passes without checking anything is a false green and is worse
than no test. If a test cannot run, mark it `#[ignore]` with an explanatory comment
AND print the loud banner when the test body runs without the required resource.

### The two-mel-frontends rule

`crates/silent-audio` houses two parameterized mel frontends:

- **TitaNet 80-band** (periodic Hann, per-feature slaney normalization) — for speaker embeddings.
- **Nemotron 128-band** (symmetric Hann, power-spectrum slaney normalization) — for ASR.

These are validated separately and must **never be unified**. A PR that
"deduplicates" them is a correctness bug. Any test that exercises mel computation
must clearly identify which frontend it is testing. See `docs/research/spike-titanet.md`
for the validation evidence.

Golden fixtures live in:

- `crates/nemotron-asr/tests/` — nemotron-asr golden clip (6.03 s, say-synthesized).
- `eval/` — TitaNet speaker-embedder bake-off fixtures (Python + JS).
- `crates/silent-core/tests/` — domain contract goldens (session, export, timestamp,
  notes, registry round-trip).

### Registry round-trip test

`crates/silent-core/tests/registry_roundtrip.rs` and `registry_real_toml.rs` verify
that `registry/models.toml` parses correctly via the `silent-core` registry types.
If you add a model to the registry, the round-trip test will catch type or schema
errors before CI does.

---

## ts-rs bindings freshness

TypeScript bindings for the wasm boundary are generated from `silent-core` using
`ts-rs` (decision: `docs/research/spike-typed-boundary.md` — ts-rs wins over tsify
on every axis that matters, including correct `bigint` for `u64`).

After any change to types in `silent-core` that carry `#[derive(TS)]`, regenerate
the bindings:

```bash
cargo test -p silent-core export_bindings
```

Then verify that no generated file changed unexpectedly:

```bash
git diff --exit-code
```

Gate 7 in `scripts/ci-local.sh` runs both steps. A stale binding is a CI failure.

---

## The registry zero-code model-add path

Adding a model within an existing engine family (another Whisper size, another Qwen
size) requires only a registry entry in `registry/models.toml` — no code changes.

Steps:

1. Add a `[[model]]` block to `registry/models.toml` with all required fields:
   `id`, `task`, `provider`, `repo`, `revision` (exact commit SHA — never `main`),
   `host`, `execution_provider`, `precision`, `memory_budget_mb`, `license`,
   `license_verified` (starts `false`), `network_origins`, `[model.cache]`,
   `[[model.files]]` (each with `path`, `size`, `sha256`, `purpose`),
   `[model.device_tiers.*]`, and `[model.ui]`.
2. Run the registry round-trip test: `cargo test -p silent-core registry`.
3. Regenerate `_headers`: `cargo run -p xtask -- gen-headers --out _headers`.
4. Commit `registry/models.toml` and `_headers` together.
5. Run `./scripts/ci-local.sh` to confirm all gates pass.

Do not add a sha256 you did not verify. Use the Hugging Face API LFS `oid` field
(which is the sha256 for LFS files) rather than downloading multi-GB files.

For non-LFS files (e.g. `tokenizer.json`), note in a comment that the sha256 is
omitted and will be verified at download time.

---

## xtask commands

`cargo run -p xtask -- <command>`:

| Command | Effect |
|---|---|
| `model-audit` | Fails if model weight files (`.onnx`, `.gguf`, `.bin`, `.safetensors`, external `.data`) are committed outside explicitly allowed tiny fixtures. |
| `gen-headers --out _headers` | Generates `_headers` from `registry/models.toml`. |
| `gen-headers --out _headers --check` | Exits non-zero if the existing `_headers` does not match what the registry would generate (CI freshness gate). |
| `gen-headers --out _headers --report-only` | Generates report-only CSP (rollback path). |
| `deploy-gate` | Fails if any file in the deploy bundle exceeds 25 MB, if model weights are present, or if `_headers` is stale. |

---

## Browser-wasm tests

`scripts/browser-wasm-tests.sh` runs the `nemotron-asr` browser test suite with
the onnxruntime-web assets vendored locally (so CI does not depend on `cdn.pyke.io`
during testing).

The script:

1. Detects the installed Chrome major/minor/build version.
2. Queries the chrome-for-testing JSON API for the highest matching chromedriver patch.
3. Downloads and caches the matching chromedriver under `vendor/`.
4. Passes `--chromedriver <path>` to `wasm-pack test --headless --chrome`.

Expected output (4 tests):

```
test mel_filterbank_shape ... ok
test mel_filterbank_values ... ok
test mel_basic ... ok
test browser_smoke ... ok
test result: ok. 4 passed; 0 failed; 0 ignored
```

The ort-web vendor server must be running (or pass `--no-vendor-server` if it is
already running on port 19999). See `scripts/vendor-ort-web.sh` to set it up.

See `docs/research/spike-ci-wasm.md` for the vendoring rationale and asset list.

---

## Workspace membership and crate responsibilities

| Crate | Edition | Published | Responsibility |
|---|---|---|---|
| `silent-core` | 2024 | No (future) | Domain contracts: commands, events, errors, registry types. Zero browser deps. |
| `silent-audio` | 2024 | No (future) | Two mel frontends + ring buffers. No ONNX, no browser deps. |
| `nemotron-asr` | 2021 | Yes (K4) | Streaming RNN-T ASR. Standalone-buildable. Stays edition 2021 (see crate comment). |
| `silent-inference` | 2024 | No | Engine traits, host adapters, Voxtral two-cap recycle, registry-driven selection. |
| `silent-diarization` | 2024 | No | TitaNet embedder, SpeakerTracker, stop-time recluster. |
| `silent-notes` | 2024 | No | NoteExtractor, OpenQs, Qwen pipeline. |
| `silent-storage` | 2024 | No | IndexedDB CRUD + Dexie v2 migration. |
| `silent-extension-sdk` | 2024 | No | Manifest schema, capability vocabulary, grant model. |
| `silent-web` | 2024 | No | wasm-bindgen boundary. All Rust surfaces the UI calls. |
| `xtask` | 2024 | No | Build tooling only. Never a dependency of any crate. |

`server/` (notetaker-server, axum) is excluded from the workspace to keep its build
byte-for-byte unchanged. It has its own `Cargo.lock`.

---

## Scope rules for contributors

- **Do not edit `_headers` by hand.** It is generated. Run `cargo run -p xtask --
  gen-headers --out _headers` and commit the result.
- **Do not add model weights to the repo.** `cargo run -p xtask -- model-audit` will
  catch this, but it is better not to commit them in the first place. Fixtures under
  `crates/nemotron-asr/tests/` are the only allowed exception, and they are tiny
  (synthetic, not production weights).
- **Do not bypass the `license_verified` flag.** Set `license_verified = false` on
  new registry entries. Mike sets it to `true` after personally reading the license.
- **Do not write mocks in product code.** A mock satisfies no acceptance criterion.
  If you cannot validate something for real, mark it `NEEDS-BROWSER-TEST` and say so.
- **The UI does not change.** The strangler-fig pattern means Rust policy runs under
  the existing `index.html`. Changes to UI behavior that are not registry-driven or
  wasm-surface-driven require explicit discussion.
