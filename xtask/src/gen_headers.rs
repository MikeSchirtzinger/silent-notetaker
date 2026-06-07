//! `xtask gen-headers` — generate the Cloudflare `_headers` file and the
//! local-server CSP from the model registry + extension grants + static
//! invariants.
//!
//! ## Design
//!
//! The generated output must semantically match what ships today in `_headers`.
//! Key decisions preserved from the shipping file (documented so future changes
//! are deliberate):
//!
//! **Preserved from shipping `_headers`:**
//! - `Cross-Origin-Opener-Policy: same-origin` — required for `SharedArrayBuffer`
//!   (multi-threaded WASM).
//! - `Cross-Origin-Embedder-Policy: require-corp` — switched from
//!   `credentialless` on 2026-06-05. The earlier "require-corp breaks HF CDN
//!   redirects" assessment (decision log 2026-06-04) was empirically disproven by
//!   `docs/research/spike-coep.md`: HF CDN satisfies require-corp via its CORS
//!   headers (CORS-eligible responses are CORP-equivalent under the COEP spec),
//!   and require-corp is the ONLY value WebKit/Safari honors for cross-origin
//!   isolation (`credentialless` leaves Safari single-threaded). The `--coep`
//!   flag emits `credentialless` for rollback.
//! - CSP is now **ENFORCED** (`Content-Security-Policy`) as of Phase 6 / R5
//!   (Task j3 — "the privacy keystone"). It shipped report-only through Phase 1–5
//!   so hidden egress could surface in dev-tools without breaking transcription;
//!   the regression sweep under enforced CSP found zero violations, so it is now
//!   the enforcement surface PRD R5 promises. The `--report-only` flag emits the
//!   old `Content-Security-Policy-Report-Only` header for rollback (re-open the
//!   observation period if a future origin regresses) without changing the
//!   directive set.
//! - `default-src 'self'` base.
//! - `script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob: <cdn-origins>` —
//!   `'unsafe-inline'` for the inline `<script>` blocks in `index.html`;
//!   `'wasm-unsafe-eval'` for `WebAssembly.instantiateStreaming`/`compile` (the
//!   wasm-pack engines compile their modules — an enforced CSP blocks WASM
//!   compilation without it; it does NOT enable JS `eval`); `blob:` for
//!   dynamically created workers. (`'wasm-unsafe-eval'` was a latent dependency
//!   masked by report-only; enforcement in Task j3 surfaced it.)
//! - `worker-src 'self' blob:` — blob: for the Nemotron/transformers workers.
//! - `connect-src`: `'self' blob: data:`, HF origins, CDN origins,
//!   and `ws://localhost:8765` (Claude bridge; decision log 2026-06-04 keeps
//!   it in hosted builds — localhost is the user's own machine).
//! - `img-src 'self' data: blob:` — blob: for screenshot thumbnails.
//! - `media-src 'self' blob:` — blob: for audio playback.
//! - `style-src 'self' 'unsafe-inline'` — inline styles in index.html.
//!
//! **Intentional differences from the shipping `_headers` comment:**
//!
//! The shipping `_headers` comment says `ws://localhost:8765` is
//! "OMITTED here (hosted build; bridge is a local-only feature)". That comment
//! was written before the 2026-06-04 decision log entry that explicitly REVERSES
//! that decision: "Hosted builds keep it in CSP connect-src (correcting v1,
//! which would have silently dropped the bridge feature from hosted
//! deployments)." The generated output ADDS `ws://localhost:8765` to match the
//! post-decision-log intent.
//!
//! `cdn.pyke.io` is included because it is a current runtime CDN origin
//! (ort-web fetches the onnxruntime-web runtime from there; the K2 vendoring
//! path exists but is dormant — the live code path still loads from the CDN).
//! `unpkg.com` is NOT included: the Dexie → Rust storage migration removed the
//! only asset it served.
//!
//! ## `--check` mode
//!
//! When `--check` is passed, the command generates into a temp buffer, diffs
//! against the on-disk `_headers`, and exits non-zero if they differ.
//!
//! ## Output targets
//!
//! - `--out <path>` — write the Cloudflare `_headers` file to `<path>`.
//! - Without `--out` — print to stdout.
//! - `--local-csp-out <path>` — also write the local-server CSP value (a single
//!   `Content-Security-Policy` header value) to `<path>` for the Axum server.

use anyhow::{Context, Result, bail};
use silent_core::registry::Registry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Arguments for `xtask gen-headers`.
#[derive(clap::Args, Debug)]
pub struct GenHeadersArgs {
    /// Path to the registry TOML file.
    /// Defaults to `<repo_root>/registry/models.toml`.
    #[arg(long)]
    pub registry: Option<PathBuf>,

    /// Write the generated `_headers` to this file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Also write the local-server CSP value to this file.
    #[arg(long)]
    pub local_csp_out: Option<PathBuf>,

    /// Check mode: diff generated content against the on-disk file.
    /// Exits non-zero if they differ.
    /// Requires `--out` to specify the file to compare against.
    #[arg(long)]
    pub check: bool,

    /// Rollback flag: emit the legacy `Content-Security-Policy-Report-Only`
    /// header (and the report-only `_headers` comment block) instead of the
    /// enforced `Content-Security-Policy`. The directive set is identical; only
    /// the enforcement posture changes. Use this to re-open the observation
    /// period if a future origin regresses under enforcement (PRD R5 / Task j3).
    #[arg(long)]
    pub report_only: bool,

    /// `Cross-Origin-Embedder-Policy` value. Defaults to `require-corp` — the
    /// only value WebKit/Safari honors for cross-origin isolation, proven
    /// HF-CDN-compatible in `docs/research/spike-coep.md`. Pass
    /// `--coep credentialless` ONLY as an emergency rollback (re-opens the
    /// Safari single-threaded regression). The COEP invariant: cross-origin
    /// fetches must remain CORS-eligible (no `no-cors` mode) — see CONTRIBUTING-RUST.md.
    #[arg(long, value_enum, default_value_t = CoepMode::RequireCorp)]
    pub coep: CoepMode,
}

/// `Cross-Origin-Embedder-Policy` value. Both yield `crossOriginIsolated` in
/// Chrome and Firefox; only `require-corp` does so in WebKit/Safari (spike-coep).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CoepMode {
    /// `require-corp` — the default and shipping posture (2026-06-05). Works in
    /// all three browser engines; satisfied by HF CDN's CORS headers.
    RequireCorp,
    /// `credentialless` — the superseded pre-2026-06-05 value, kept ONLY as a
    /// rollback. Leaves WebKit/Safari single-threaded (`crossOriginIsolated=false`).
    Credentialless,
}

impl CoepMode {
    /// The header value string for this COEP mode.
    fn header_value(self) -> &'static str {
        match self {
            CoepMode::RequireCorp => "require-corp",
            CoepMode::Credentialless => "credentialless",
        }
    }

    /// The explanatory `_headers` comment block for this COEP posture.
    fn comment_block(self) -> &'static str {
        match self {
            CoepMode::RequireCorp => {
                "# COEP=require-corp (switched from credentialless 2026-06-05):\n\
                 # require-corp is the ONLY value WebKit/Safari honors for cross-origin\n\
                 # isolation; credentialless left Safari single-threaded. HF CDN\n\
                 # satisfies require-corp via its CORS headers (CORS-eligible responses\n\
                 # are CORP-equivalent under the spec) — the earlier \"breaks HF CDN\"\n\
                 # claim was empirically disproven. Evidence: docs/research/spike-coep.md.\n\
                 # INVARIANT: cross-origin fetches must stay CORS-eligible (no no-cors)."
            }
            CoepMode::Credentialless => {
                "# COEP=credentialless (ROLLBACK from require-corp via --coep credentialless):\n\
                 # this leaves WebKit/Safari single-threaded (crossOriginIsolated=false).\n\
                 # Only use to recover from an HF-CDN CORS regression. See\n\
                 # docs/research/spike-coep.md for why require-corp is the default."
            }
        }
    }
}

/// CSP enforcement posture. The directive *value* is identical in both; only the
/// header name (and the explanatory comment block) differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CspMode {
    /// `Content-Security-Policy` — the browser BLOCKS violating requests. This is
    /// the Phase 6 / R5 default: CSP is the privacy-enforcement surface.
    Enforced,
    /// `Content-Security-Policy-Report-Only` — the browser REPORTS but does not
    /// block. The pre-Phase-6 posture, kept as a `--report-only` rollback.
    ReportOnly,
}

impl CspMode {
    /// The HTTP response header name for this mode.
    fn header_name(self) -> &'static str {
        match self {
            CspMode::Enforced => "Content-Security-Policy",
            CspMode::ReportOnly => "Content-Security-Policy-Report-Only",
        }
    }

    /// The one-line `_headers` comment describing the current posture.
    fn comment_line(self) -> &'static str {
        match self {
            CspMode::Enforced => {
                "# CSP is ENFORCED (Phase 6 / R5 — the privacy keystone, Task j3)."
            }
            CspMode::ReportOnly => {
                "# CSP is REPORT-ONLY (rollback via --report-only; not enforced)."
            }
        }
    }
}

/// Static CDN origins always included in the CSP, regardless of registry
/// content. These cover the current runtime dependencies:
/// - jsdelivr: transformers.js scripts
/// - cdn.pyke.io: ort-web's onnxruntime-web runtime loader
///
/// `unpkg.com` was REMOVED with the Dexie → Rust storage migration (the only
/// thing it ever served); see storage-engine.js. The vendoring decision (Task
/// K2) will remove cdn.pyke.io if/when it wires the vendored assets into the
/// deploy bundle. Until then, it stays.
const STATIC_CDN_ORIGINS: &[&str] = &["https://cdn.jsdelivr.net", "https://cdn.pyke.io"];

/// Hugging Face origins always included. Covers the CDN + regional LFS variants
/// that HF redirects to (observed in production network panel — the `_headers`
/// file already lists these):
const HF_ORIGINS: &[&str] = &[
    "https://huggingface.co",
    "https://*.hf.co",
    "https://cdn-lfs.huggingface.co",
    "https://cdn-lfs-us-1.huggingface.co",
];

/// The Claude bridge WebSocket endpoint. Included in hosted builds per the
/// 2026-06-04 decision log: "Hosted builds KEEP it in CSP connect-src."
const BRIDGE_ORIGIN: &str = "ws://localhost:8765";

/// Run `xtask gen-headers`.
pub fn run(args: GenHeadersArgs) -> Result<()> {
    let repo_root = resolve_repo_root()?;
    let registry_path = args
        .registry
        .unwrap_or_else(|| repo_root.join("registry").join("models.toml"));

    let mode = if args.report_only {
        CspMode::ReportOnly
    } else {
        CspMode::Enforced
    };
    let coep = args.coep;

    let registry = load_registry_optional(&registry_path)?;
    let content = generate_headers_with_mode(&registry, mode, coep);

    if args.check {
        // --check mode: compare generated vs on-disk.
        let target_path = args
            .out
            .as_ref()
            .context("--check requires --out=<path to compare against>")?;
        check_freshness(target_path, &content)?;
    } else if let Some(out_path) = &args.out {
        std::fs::write(out_path, &content)
            .with_context(|| format!("writing _headers to {}", out_path.display()))?;
        eprintln!(
            "[gen-headers] wrote: {} ({:?} CSP, {:?} COEP)",
            out_path.display(),
            mode,
            coep
        );
    } else {
        print!("{content}");
    }

    if let Some(csp_out) = &args.local_csp_out {
        let csp_value = generate_local_csp_value(&registry);
        std::fs::write(csp_out, &csp_value)
            .with_context(|| format!("writing local CSP to {}", csp_out.display()))?;
        eprintln!("[gen-headers] wrote local CSP: {}", csp_out.display());
    }

    Ok(())
}

/// Load the registry from the given path.
///
/// If the file does not exist, returns an empty `Registry` and logs a warning
/// (the CSP can still be generated from static invariants; E1 will wire the
/// real registry). This allows D2 to test gen-headers before D1 populates the
/// registry, as required by the orchestrator decision.
fn load_registry_optional(registry_path: &Path) -> Result<Registry> {
    if !registry_path.exists() {
        eprintln!(
            "[gen-headers] registry not found at {} — using static invariants only \
             (D1 will populate the registry; use --registry=<fixture> during development)",
            registry_path.display()
        );
        return Ok(Registry::default());
    }
    let content = std::fs::read_to_string(registry_path)
        .with_context(|| format!("reading registry: {}", registry_path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("parsing registry TOML: {}", registry_path.display()))
}

/// Collect all unique `network_origins` from the registry.
fn registry_origins(registry: &Registry) -> BTreeSet<String> {
    let mut origins = BTreeSet::new();
    for model in &registry.models {
        for origin in &model.network_origins {
            origins.insert(origin.clone());
        }
    }
    origins
}

/// Build the full `connect-src` directive value.
///
/// Sources (in order, deduped):
/// 1. `'self' blob: data:` — static invariants.
/// 2. CDN origins (jsdelivr, cdn.pyke.io) — static runtime deps.
/// 3. HF origins — static, always needed for model fetch.
/// 4. Registry-derived `network_origins` — any additional origins from models.
/// 5. Claude bridge — `ws://localhost:8765`.
fn build_connect_src(registry: &Registry) -> String {
    let mut parts: Vec<String> = vec!["'self'".into(), "blob:".into(), "data:".into()];

    // Static CDN origins.
    for origin in STATIC_CDN_ORIGINS {
        parts.push((*origin).to_owned());
    }

    // Hugging Face origins (always included).
    for origin in HF_ORIGINS {
        parts.push((*origin).to_owned());
    }

    // Registry-derived origins (deduped via BTreeSet, sorted for determinism).
    let reg_origins = registry_origins(registry);
    for origin in reg_origins {
        // Skip duplicates already covered by the static lists above.
        if !parts.contains(&origin) {
            parts.push(origin);
        }
    }

    // Claude bridge — always last for visual clarity.
    if !parts.contains(&BRIDGE_ORIGIN.to_owned()) {
        parts.push(BRIDGE_ORIGIN.to_owned());
    }

    parts.join(" ")
}

/// Build the full CSP directive value (a single long line).
///
/// `'wasm-unsafe-eval'` in `script-src` is REQUIRED for the app to run at all:
/// every wasm-pack engine (`silent-web`, `nemotron-asr`, …) compiles its module
/// via `WebAssembly.instantiateStreaming`/`compile`, which an enforced CSP blocks
/// unless `script-src` permits wasm compilation. The narrow `'wasm-unsafe-eval'`
/// token allows WASM compilation ONLY — it does NOT enable JS `eval()` (that is
/// the broader `'unsafe-eval'`, which is deliberately NOT granted). This was a
/// latent dependency masked by report-only mode; enforcing CSP (Task j3) surfaced
/// it, and it is fixed here in the generator (NOT hand-edited) so `_headers` and
/// the local server stay in lockstep. Cross-origin isolation (COOP/COEP) exists
/// precisely so this multithreaded WASM runs — the token is intrinsic to the app.
fn build_csp(registry: &Registry) -> String {
    let connect_src = build_connect_src(registry);
    // Script CDN origins (same set as the static CDNs, no HF).
    let script_cdns = STATIC_CDN_ORIGINS.join(" ");
    format!(
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob: {script_cdns}; \
         worker-src 'self' blob:; \
         connect-src {connect_src}; \
         frame-src 'self'; \
         img-src 'self' data: blob:; \
         media-src 'self' blob:; \
         style-src 'self' 'unsafe-inline'"
    )
}

/// Generate the full `_headers` file content for the shipping posture: *enforced*
/// CSP (Phase 6 / R5) + *require-corp* COEP (spike-coep, 2026-06-05). `deploy-gate`
/// calls this for its freshness check, so the shipped `_headers` must be exactly
/// this output.
pub fn generate_headers(registry: &Registry) -> String {
    generate_headers_with_mode(registry, CspMode::Enforced, CoepMode::RequireCorp)
}

/// Generate the full `_headers` file content for explicit [`CspMode`] +
/// [`CoepMode`] postures.
pub fn generate_headers_with_mode(registry: &Registry, mode: CspMode, coep: CoepMode) -> String {
    let csp = build_csp(registry);
    let header_name = mode.header_name();
    let comment_line = mode.comment_line();
    let coep_value = coep.header_value();
    let coep_comment = coep.comment_block();
    format!(
        "# Cloudflare Pages / Netlify response headers.\n\
         # GENERATED by `xtask gen-headers` — do not edit by hand.\n\
         # Re-generate with: cargo xtask gen-headers --out _headers\n\
         # Check freshness: cargo xtask gen-headers --out _headers --check\n\
         # Rollback to report-only: cargo xtask gen-headers --out _headers --report-only\n\
         # Rollback COEP to credentialless: cargo xtask gen-headers --out _headers --coep credentialless\n\
         #\n\
         # Cross-origin isolation → crossOriginIsolated → SharedArrayBuffer →\n\
         # multithreaded WASM.\n\
         #\n\
         {coep_comment}\n\
         #\n\
         {comment_line}\n\
         # Per-extension connect-src relaxations are applied to the extension's\n\
         # own sandboxed-iframe context (GrantSet::connect_src) — NOT here. This\n\
         # BASE page policy contains no extension origins.\n\
         # ws://localhost:8765 (Claude bridge) is KEPT in hosted builds —\n\
         # decision log 2026-06-04: localhost is inside the user's trust boundary.\n\
         /*\n\
           Cross-Origin-Opener-Policy: same-origin\n\
           Cross-Origin-Embedder-Policy: {coep_value}\n\
           {header_name}: {csp}\n"
    )
}

/// Generate the local-server `Content-Security-Policy` header VALUE (not the
/// full `_headers` file — just the directive value for the Axum `server/`
/// crate's enforced CSP header).
///
/// For the local server we emit the same policy but as an **enforcing** header
/// (not report-only) because the local dev server does not need the observation
/// period.
pub fn generate_local_csp_value(registry: &Registry) -> String {
    build_csp(registry)
}

/// Compare generated content to the on-disk file and bail on drift.
fn check_freshness(target: &Path, generated: &str) -> Result<()> {
    if !target.exists() {
        bail!(
            "[gen-headers --check] target file not found: {} \
             (generate it first with: cargo xtask gen-headers --out {})",
            target.display(),
            target.display()
        );
    }
    let on_disk =
        std::fs::read_to_string(target).with_context(|| format!("reading {}", target.display()))?;

    if on_disk == generated {
        eprintln!(
            "[gen-headers --check] PASS — {} is up to date.",
            target.display()
        );
        Ok(())
    } else {
        // Emit a human-readable diff (line-by-line) so CI output is actionable.
        eprintln!(
            "[gen-headers --check] FAIL — {} is STALE. Diff (< on-disk, > generated):",
            target.display()
        );
        diff_lines(&on_disk, generated);
        bail!(
            "_headers is stale relative to the registry; \
             regenerate with: cargo xtask gen-headers --out {}",
            target.display()
        )
    }
}

/// Print a simple line-level diff to stderr.
fn diff_lines(old: &str, new: &str) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let max = old_lines.len().max(new_lines.len());
    for i in 0..max {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(o), Some(n)) if o == n => {}
            (Some(o), Some(n)) => {
                eprintln!("< {o}");
                eprintln!("> {n}");
            }
            (Some(o), None) => eprintln!("< {o}"),
            (None, Some(n)) => eprintln!("> {n}"),
            (None, None) => {}
        }
    }
}

fn resolve_repo_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let xtask_dir = PathBuf::from(manifest_dir);
    xtask_dir
        .parent()
        .context("xtask CARGO_MANIFEST_DIR has no parent — unexpected layout")
        .map(Path::to_path_buf)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as assertion mechanism"
)]
mod tests {
    use super::*;
    use silent_core::ids::ModelId;
    use silent_core::registry::{
        Cache, CacheStore, ExecutionProvider, Host, Model, ModelFile, Provider, Task,
    };

    fn make_minimal_registry() -> Registry {
        let model = Model {
            id: ModelId::new("asr.test.fixture"),
            task: Task::Asr,
            provider: Provider::Huggingface,
            repo: "owner/test-repo".into(),
            revision: "abc1234567890abcdef1234567890abcdef12345".into(),
            host: Host::RustOrtWeb,
            execution_provider: ExecutionProvider::Cpu,
            precision: vec!["int8".into()],
            memory_budget_mb: 100,
            cache: Cache::new(CacheStore::CacheApi),
            license: "MIT".into(),
            license_verified: true,
            network_origins: vec!["https://huggingface.co".into()],
            files: vec![ModelFile {
                path: "model.onnx".into(),
                size: Some(1024),
                sha256: Some("deadbeef".repeat(8)),
                purpose: Some("encoder".into()),
            }],
            device_tiers: std::collections::BTreeMap::default(),
            validation: None,
            perf_budget: None,
            ui: None,
            composite_of: Vec::new(),
        };
        Registry {
            models: vec![model],
        }
    }

    #[test]
    fn generated_headers_contains_invariants() {
        let registry = make_minimal_registry();
        let headers = generate_headers(&registry);

        // COOP/COEP invariants.
        assert!(
            headers.contains("Cross-Origin-Opener-Policy: same-origin"),
            "missing COOP: {headers}"
        );
        // COEP is require-corp by default (spike-coep, 2026-06-05) — NOT
        // credentialless, which left Safari single-threaded.
        assert!(
            headers.contains("Cross-Origin-Embedder-Policy: require-corp"),
            "missing require-corp COEP: {headers}"
        );
        assert!(
            !headers.contains("Cross-Origin-Embedder-Policy: credentialless"),
            "default headers must not ship credentialless COEP anymore: {headers}"
        );

        // ENFORCED (Phase 6 / R5 / Task j3) — not report-only.
        assert!(
            headers.contains("Content-Security-Policy: "),
            "should be the ENFORCED CSP header: {headers}"
        );
        assert!(
            !headers.contains("Content-Security-Policy-Report-Only"),
            "should NOT be report-only by default anymore (j3 enforced it): {headers}"
        );

        // Bridge origin present.
        assert!(
            headers.contains("ws://localhost:8765"),
            "missing bridge origin: {headers}"
        );

        // The base page policy must NOT carry any per-extension origin — those
        // are applied only to the extension's own iframe context (R7 / j1 notes).
        assert!(
            !headers.contains("api.notion.com"),
            "base page CSP must not contain extension origins: {headers}"
        );
    }

    #[test]
    fn report_only_mode_emits_legacy_header_same_directives() {
        let registry = make_minimal_registry();
        let enforced =
            generate_headers_with_mode(&registry, CspMode::Enforced, CoepMode::RequireCorp);
        let report_only =
            generate_headers_with_mode(&registry, CspMode::ReportOnly, CoepMode::RequireCorp);

        assert!(
            report_only.contains("Content-Security-Policy-Report-Only: "),
            "rollback should emit report-only header: {report_only}"
        );
        // The directive VALUE must be identical across modes — only the posture
        // (header name + comment) changes.
        let enforced_csp = build_csp(&registry);
        assert!(
            enforced.contains(&format!("Content-Security-Policy: {enforced_csp}")),
            "enforced header missing canonical directive value"
        );
        assert!(
            report_only.contains(&format!(
                "Content-Security-Policy-Report-Only: {enforced_csp}"
            )),
            "report-only header missing the SAME canonical directive value"
        );
    }

    #[test]
    fn coep_default_is_require_corp_rollback_is_credentialless() {
        let registry = make_minimal_registry();
        let require_corp =
            generate_headers_with_mode(&registry, CspMode::Enforced, CoepMode::RequireCorp);
        let credentialless =
            generate_headers_with_mode(&registry, CspMode::Enforced, CoepMode::Credentialless);

        // Default (require-corp) — the spike-coep posture; the only value WebKit
        // honors for cross-origin isolation.
        assert!(
            require_corp.contains("Cross-Origin-Embedder-Policy: require-corp"),
            "default must emit require-corp: {require_corp}"
        );
        assert!(
            !require_corp.contains("Cross-Origin-Embedder-Policy: credentialless"),
            "require-corp output must not contain credentialless value"
        );

        // Rollback (credentialless) — still available for an HF-CDN CORS regression.
        assert!(
            credentialless.contains("Cross-Origin-Embedder-Policy: credentialless"),
            "rollback must emit credentialless: {credentialless}"
        );
        assert!(
            !credentialless.contains("Cross-Origin-Embedder-Policy: require-corp"),
            "credentialless output must not contain require-corp value"
        );

        // The CSP directive set and COOP are identical across COEP modes — only
        // the COEP value + its comment block change.
        assert!(
            credentialless.contains("Cross-Origin-Opener-Policy: same-origin"),
            "COOP must be unchanged by the COEP rollback"
        );
        let csp = build_csp(&registry);
        assert!(require_corp.contains(&csp) && credentialless.contains(&csp));
    }

    #[test]
    fn connect_src_includes_hf_and_cdn() {
        let registry = make_minimal_registry();
        let csp = build_csp(&registry);
        assert!(csp.contains("https://huggingface.co"), "missing HF: {csp}");
        assert!(csp.contains("https://*.hf.co"), "missing *.hf.co: {csp}");
        assert!(
            csp.contains("https://cdn-lfs.huggingface.co"),
            "missing cdn-lfs: {csp}"
        );
        assert!(
            csp.contains("https://cdn-lfs-us-1.huggingface.co"),
            "missing cdn-lfs-us-1: {csp}"
        );
        assert!(
            csp.contains("https://cdn.jsdelivr.net"),
            "missing jsdelivr: {csp}"
        );
        assert!(
            !csp.contains("https://unpkg.com"),
            "stale unpkg (removed with Dexie) must not reappear: {csp}"
        );
        assert!(
            csp.contains("https://cdn.pyke.io"),
            "missing cdn.pyke.io: {csp}"
        );
        assert!(csp.contains("ws://localhost:8765"), "missing bridge: {csp}");
    }

    #[test]
    fn base_csp_allows_self_framing_for_ext_route() {
        // The per-extension document route (`/ext/<name>/`, Task j2b) is
        // same-origin and must be framable from the base page; `frame-src 'self'`
        // authorizes exactly that and nothing cross-origin. The base CSP carries
        // NO extension network origins — those live only on the ext route's own
        // response-header CSP.
        let registry = make_minimal_registry();
        let csp = build_csp(&registry);
        assert!(
            csp.contains("frame-src 'self'"),
            "missing frame-src 'self': {csp}"
        );
        assert!(
            !csp.contains("api.notion.com"),
            "base CSP must not carry extension origins: {csp}"
        );
    }

    #[test]
    fn clean_fixture_registry_generates_valid_headers() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clean/models.toml");
        let content = std::fs::read_to_string(&fixture).expect("read clean fixture");
        let registry: Registry = toml::from_str(&content).expect("parse clean fixture");
        let headers = generate_headers(&registry);
        assert!(
            headers.contains("Cross-Origin-Opener-Policy: same-origin"),
            "missing COOP: {headers}"
        );
    }

    #[test]
    fn check_mode_passes_on_matching_content() {
        use std::io::Write;
        let registry = make_minimal_registry();
        let generated = generate_headers(&registry);

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(generated.as_bytes()).expect("write");

        // check_freshness should return Ok when content matches.
        check_freshness(tmp.path(), &generated).expect("should pass on matching content");
    }

    #[test]
    fn check_mode_fails_on_stale_content() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"stale content").expect("write");

        let result = check_freshness(tmp.path(), "fresh generated content");
        assert!(result.is_err(), "should fail on mismatched content");
    }
}
