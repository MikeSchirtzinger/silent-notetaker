//! `xtask` — build/audit tooling for Silent Notetaker (Task D2).
//!
//! Subcommands:
//!
//! - [`model_audit`] — fail on committed model weights outside the tiny-fixture
//!   allowlist; require pinned revisions (no `main`), per-file sha256 + size,
//!   `license` and `license_verified` on every registry entry. Currently FAILS
//!   on the committed `titanet.onnx` / `mel_fb.json` — that is correct; E1
//!   removes them.
//!
//! - [`gen_headers`] — generate the Cloudflare `_headers` file and the
//!   local-server CSP from the registry's `network_origins` plus the static
//!   invariants (COOP/COEP/report-only CSP). `--check` mode diffs generated vs
//!   on-disk and exits non-zero on drift.
//!
//! - [`deploy_gate`] — given a bundle directory, fail on: any model-weight
//!   file, any file >25 MB (Cloudflare Pages limit), or a stale `_headers`
//!   (via `gen-headers --check`). Tests use planted violations in tempdirs.
//!
//! `anyhow` is acceptable at this binary boundary (PRD "Core contracts" and
//! workspace `Cargo.toml` comment).

mod deploy_gate;
mod gen_headers;
mod model_audit;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Silent Notetaker build-and-audit tooling.
///
/// Run via `cargo xtask <subcommand>` (the workspace `.cargo/config.toml` wires
/// this as an alias, or call the binary directly).
#[derive(Parser, Debug)]
#[command(name = "xtask", author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Scan the repo for committed model weights and audit the registry for
    /// completeness (pinned revisions, hashes, sizes, licenses).
    ///
    /// CURRENTLY FAILS on `titanet.onnx` + `mel_fb.json` — that is correct
    /// behavior; they are removed in Task E1.
    ModelAudit(model_audit::ModelAuditArgs),

    /// Generate `_headers` (Cloudflare) and the local-server CSP from the
    /// registry + extension grants + static invariants.
    ///
    /// `--check` mode: diff generated vs on-disk; exit non-zero on drift.
    GenHeaders(gen_headers::GenHeadersArgs),

    /// Gate a deploy bundle: fail on weight files, files >25 MB, or stale
    /// `_headers` (delegates to gen-headers --check).
    DeployGate(deploy_gate::DeployGateArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::ModelAudit(args) => model_audit::run(args),
        Command::GenHeaders(args) => gen_headers::run(args),
        Command::DeployGate(args) => deploy_gate::run(args),
    }
}
