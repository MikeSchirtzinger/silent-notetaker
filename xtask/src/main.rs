//! `xtask` — build/audit tooling for Silent Notetaker (stub — Task D2).
//!
//! Implemented in Phase 1 (Task D2):
//! - `model-audit`  — fail on committed weights outside allowed tiny fixtures;
//!   require pinned revisions, hashes, sizes, licenses, `license_verified`.
//! - `gen-headers`  — generate `_headers` + local-server CSP from the registry
//!   plus extension grants; `--check` mode for CI freshness.
//! - `deploy-gate`  — weight-free bundle, 25 MB/file Cloudflare limit, headers
//!   freshness.
//!
//! `anyhow` is acceptable at this binary boundary (PRD "Core contracts"); it is
//! added with the real implementation in Task D2.
//!
//! This stub reserves the crate in the workspace so Phase D agents implement the
//! subcommands without touching the root `Cargo.toml`.

fn main() {
    // Touch silent-core so the dependency edge is real (the subcommands will
    // read `silent_core::registry` types). Keeps the stub honest about its
    // intended coupling without implementing any logic yet.
    let version = silent_core::BOUNDARY_CONTRACT_VERSION;
    eprintln!(
        "xtask (stub — Task D2). boundary contract v{version}. \
         planned subcommands: model-audit | gen-headers | deploy-gate"
    );
    eprintln!("not yet implemented; see xtask/Cargo.toml and spec Task D2.");
}
