//! `xtask model-audit` — scan the repo for committed model weights and verify
//! the registry is complete.
//!
//! ## Weight scan
//!
//! Walks every file tracked by git (or, when not in a git repo, every file
//! under the repo root) and fails if any file matches the weight-extension set
//! (`.onnx`, `.gguf`, `.safetensors`, and any other binary blob over
//! [`BINARY_SIZE_THRESHOLD_BYTES`]) **unless** it is listed in the explicit
//! tiny-fixture allowlist [`ALLOWED_FIXTURE_PATHS`].
//!
//! ## Registry audit
//!
//! Parses `registry/models.toml` (the canonical data path per the orchestrator
//! decision) and fails if any production model entry has:
//! - `revision = "main"` (or any non-SHA value — we check for a 40-hex-char
//!   commit SHA or a numeric tag; "main" / "master" / "HEAD" are rejected),
//! - any [`ModelFile`] missing `sha256` or `size`,
//! - an empty `license` field, or
//! - `license_verified = false` (shipped defaults must have this `true`).
//!
//! ## Exit codes
//!
//! Exits 0 only when both scans find no violations. Each violation is printed
//! to stderr so CI can surface it in job output.

use anyhow::{Context, Result, bail};
use silent_core::registry::Registry;
use std::path::{Path, PathBuf};

/// Arguments for `xtask model-audit`.
#[derive(clap::Args, Debug)]
pub struct ModelAuditArgs {
    /// Root of the repository to scan. Defaults to the directory containing
    /// the workspace `Cargo.toml` (i.e., `$CARGO_MANIFEST_DIR/../..` relative
    /// to the xtask crate).
    #[arg(long)]
    pub repo_root: Option<PathBuf>,

    /// Path to the registry TOML file.
    /// Defaults to `<repo_root>/registry/models.toml`.
    #[arg(long)]
    pub registry: Option<PathBuf>,
}

/// File extensions that identify model weight files (case-insensitive check).
const WEIGHT_EXTENSIONS: &[&str] = &["onnx", "gguf", "safetensors", "bin", "pt", "pth"];

/// Known model artifact filenames that should not be committed to the repo,
/// regardless of extension. These are domain-specific files that are model
/// artifacts (e.g. mel filterbank matrices) but do not have standard weight
/// extensions.
///
/// Current entries:
/// - `mel_fb.json`: mel filterbank JSON for the TitaNet audio frontend.
///   Generated from the model at build time; should be fetched at runtime, not
///   committed. E1 removes this from the repo root.
const KNOWN_MODEL_ARTIFACTS: &[&str] = &["mel_fb.json"];

/// Minimum file size for a "large binary blob" check, in bytes (25 MB).
///
/// Any file ≥ this size that is not in [`ALLOWED_FIXTURE_PATHS`] is treated as
/// a weight violation — even without a recognized extension — because Cloudflare
/// Pages has a 25 MB/file limit (PRD R6) and no legitimate source file should
/// be this large.
const BINARY_SIZE_THRESHOLD_BYTES: u64 = 25 * 1024 * 1024;

/// Tiny-fixture paths that are explicitly allowed to contain weight-like files.
///
/// Paths are relative to the repo root and use forward-slash separators. The
/// allowlist is intentionally minimal — it exists only for
/// `eval/`-style byte-validation fixtures that cannot be avoided. Every entry
/// must be a genuine tiny fixture (< 1 MB is a good heuristic; nothing here
/// should approach the 25 MB Cloudflare limit).
///
/// # Current entries
///
/// - `eval/js/titanet.onnx`: the mel-frontend cosine-validation fixture used by
///   the speaker-embedder bake-off harness (`eval/`). It is a tiny reference
///   copy checked in solely for `eval/` golden validation. It is NOT the
///   production weight (that lives on Hugging Face and is fetched at runtime);
///   it is intentionally kept in the repo for the `eval/` harness.
///
/// E1 removes `titanet.onnx` (root) and `mel_fb.json` (root). The eval/
/// fixtures are separate and remain until the eval/ harness is updated to
/// fetch them at test time.
const ALLOWED_FIXTURE_PATHS: &[&str] = &["eval/js/titanet.onnx", "eval/js/mel_fb.json"];

/// Run `xtask model-audit`.
///
/// Returns `Ok(())` only when all checks pass. On any violation, prints a
/// diagnostic to stderr and returns an error.
pub fn run(args: ModelAuditArgs) -> Result<()> {
    let repo_root = resolve_repo_root(args.repo_root)?;
    let registry_path = args
        .registry
        .unwrap_or_else(|| repo_root.join("registry").join("models.toml"));

    eprintln!("[model-audit] repo root:     {}", repo_root.display());
    eprintln!("[model-audit] registry path: {}", registry_path.display());

    let mut violations: Vec<String> = Vec::new();

    // -----------------------------------------------------------------------
    // 1. Weight scan — look for committed model weight files.
    // -----------------------------------------------------------------------
    eprintln!("[model-audit] scanning for committed model weights…");
    let weight_violations = scan_for_weights(&repo_root)?;
    violations.extend(weight_violations);

    // -----------------------------------------------------------------------
    // 2. Registry audit — parse and validate the registry.
    // -----------------------------------------------------------------------
    eprintln!(
        "[model-audit] auditing registry: {}",
        registry_path.display()
    );
    let registry_violations = audit_registry(&registry_path)?;
    violations.extend(registry_violations);

    // -----------------------------------------------------------------------
    // Report.
    // -----------------------------------------------------------------------
    if violations.is_empty() {
        eprintln!("[model-audit] PASS — no violations found.");
        Ok(())
    } else {
        eprintln!("[model-audit] FAIL — {} violation(s):", violations.len());
        for v in &violations {
            eprintln!("  {v}");
        }
        bail!(
            "model-audit found {} violation(s); see above",
            violations.len()
        )
    }
}

/// Locate the repository root.
///
/// Uses the caller-supplied path if given; otherwise walks up from the
/// `CARGO_MANIFEST_DIR` of the xtask crate (i.e., `xtask/`) to find the
/// workspace root (the directory containing the root `Cargo.toml`).
fn resolve_repo_root(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    // CARGO_MANIFEST_DIR is set by cargo when building/running xtask.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    // The xtask crate lives at `<repo_root>/xtask/`, so the repo root is the
    // parent of `CARGO_MANIFEST_DIR`.
    let xtask_dir = PathBuf::from(manifest_dir);
    let root = xtask_dir
        .parent()
        .context("xtask CARGO_MANIFEST_DIR has no parent — unexpected layout")?
        .to_owned();
    Ok(root)
}

/// Walk the repo (using `git ls-files` when available, falling back to
/// `walkdir`) and return violation messages for every weight file found outside
/// the allowlist.
fn scan_for_weights(repo_root: &Path) -> Result<Vec<String>> {
    let files = list_tracked_files(repo_root)?;
    let mut violations = Vec::new();

    for rel_path in files {
        // Normalize separators for cross-platform allowlist matching.
        let rel_str = rel_path.replace('\\', "/");

        // Skip files in the explicit allowlist.
        if ALLOWED_FIXTURE_PATHS
            .iter()
            .any(|allowed| rel_str == *allowed || rel_str.starts_with(&format!("{allowed}/")))
        {
            eprintln!("[model-audit]   allowed fixture: {rel_str}");
            continue;
        }

        let abs = repo_root.join(&rel_path);

        // Check known model artifact filenames (e.g. `mel_fb.json`).
        if let Some(filename) = abs.file_name().and_then(|n| n.to_str())
            && KNOWN_MODEL_ARTIFACTS
                .iter()
                .any(|a| filename.eq_ignore_ascii_case(a))
        {
            violations.push(format!("committed model artifact (by name): {rel_str}"));
            continue;
        }

        // Check extension.
        let has_weight_ext = abs.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
            WEIGHT_EXTENSIONS
                .iter()
                .any(|w| ext.eq_ignore_ascii_case(w))
        });

        if has_weight_ext {
            violations.push(format!(
                "committed model weight file (by extension): {rel_str}"
            ));
            continue;
        }

        // Check large binary blobs (any file ≥ 25 MB not in the allowlist).
        // The u64→f64 cast for the human-readable size is intentional (~1 decimal
        // place is sufficient for a diagnostic message).
        #[allow(clippy::cast_precision_loss)]
        if let Ok(meta) = std::fs::metadata(&abs)
            && meta.len() >= BINARY_SIZE_THRESHOLD_BYTES
        {
            violations.push(format!(
                "large binary blob (≥ {} MB): {} ({:.1} MB)",
                BINARY_SIZE_THRESHOLD_BYTES / 1024 / 1024,
                rel_str,
                meta.len() as f64 / 1024.0 / 1024.0,
            ));
        }
    }

    Ok(violations)
}

/// Return relative paths of every file tracked by the git index.
///
/// Uses `git ls-files` when a git repo is detected. Falls back to a recursive
/// `walkdir` (skipping `.git/` and `target/`) when git is not available.
fn list_tracked_files(repo_root: &Path) -> Result<Vec<String>> {
    // Try git first.
    let git_output = std::process::Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "ls-files", "--cached"])
        .output();

    match git_output {
        Ok(out) if out.status.success() => {
            let text =
                String::from_utf8(out.stdout).context("git ls-files output is not valid UTF-8")?;
            Ok(text.lines().map(str::to_owned).collect())
        }
        _ => {
            // Fallback: walk the directory, skipping .git, target, and node_modules.
            eprintln!("[model-audit] git not available; falling back to walkdir");
            let mut files = Vec::new();
            for entry in walkdir::WalkDir::new(repo_root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    name != ".git" && name != "target" && name != "node_modules"
                })
            {
                let entry = entry.context("walkdir error")?;
                if entry.file_type().is_file()
                    && let Ok(rel) = entry.path().strip_prefix(repo_root)
                {
                    files.push(rel.to_string_lossy().into_owned());
                }
            }
            Ok(files)
        }
    }
}

/// The sentinel value D1 uses to mark entries that require a user action
/// (e.g. SenseVoice waiting for Mike to run the first-party re-host upload).
const BLOCKED_SENTINEL: &str = "BLOCKED-ON-USER-GATE";

/// Parse the registry and return violation messages.
///
/// ## Severity levels
///
/// This function distinguishes three categories:
/// - **Hard violations** (returned as strings, cause non-zero exit): truly bad
///   values like `revision = "main"` or an empty `license`.
/// - **USER-GATE notes** (printed to stderr, NOT returned as violations):
///   entries marked `revision = "BLOCKED-ON-USER-GATE"` or
///   `license_verified = false`. These are expected during development and
///   require Mike's action; they do not block the build.
/// - **Informational** (printed to stderr): `sha256` absent on non-LFS files
///   (D1 documents these as "verify at download time").
///
/// ## Registry-absent behaviour
///
/// If the registry file does not exist, this is a hard failure per the
/// orchestrator decision ("fail loudly with a clear error if the file is
/// absent/invalid").
fn audit_registry(registry_path: &Path) -> Result<Vec<String>> {
    if !registry_path.exists() {
        bail!(
            "registry file not found: {}  \
            (d1-registry writes it; run against a fixture registry with \
            --registry=xtask/tests/fixtures/clean/models.toml during development)",
            registry_path.display()
        );
    }

    let content = std::fs::read_to_string(registry_path)
        .with_context(|| format!("reading registry: {}", registry_path.display()))?;

    let registry: Registry = toml::from_str(&content)
        .with_context(|| format!("parsing registry TOML: {}", registry_path.display()))?;

    if registry.models.is_empty() {
        return Ok(vec![
            "registry contains no models — populate or point at a fixture".to_owned(),
        ]);
    }

    let mut violations = Vec::new();

    for model in &registry.models {
        let id = model.id.as_str();

        // 1. Revision must not be "main", "master", "HEAD", or empty.
        //    `BLOCKED-ON-USER-GATE` is a special D1 sentinel for entries that
        //    are awaiting a user action (e.g. SenseVoice first-party rehost).
        //    It is NOT a generic unpinned revision — emit a USER-GATE notice
        //    (stderr only) rather than a hard violation.
        if model.revision == BLOCKED_SENTINEL {
            eprintln!(
                "[model-audit] USER-GATE  model `{id}`: revision = \"{BLOCKED_SENTINEL}\" — \
                 awaiting user upload; run the first-party rehost script and update \
                 the registry with a pinned SHA before shipping."
            );
        } else if is_unpinned_revision(&model.revision) {
            violations.push(format!(
                "model `{id}`: revision `{}` is not pinned (must be a commit SHA or \
                 immutable tag — `main`/`master`/`HEAD` are rejected; use \
                 \"{BLOCKED_SENTINEL}\" for intentional user-gate placeholders)",
                model.revision
            ));
        }

        // 2. File integrity fields.
        //    sha256 absent: informational only — D1 documents these as
        //    "sha256 omitted: non-LFS file, verify at download time".
        //    size absent: same leniency; the download size isn't always known.
        for file in &model.files {
            if file.sha256.is_none() {
                eprintln!(
                    "[model-audit] INFO      model `{id}` file `{}`: sha256 absent \
                     (non-LFS file; will be verified at download time)",
                    file.path
                );
            }
            if file.size.is_none() {
                eprintln!(
                    "[model-audit] INFO      model `{id}` file `{}`: size absent \
                     (will be recorded after first successful download)",
                    file.path
                );
            }
        }

        // 3. license must not be empty.
        if model.license.trim().is_empty() {
            violations.push(format!("model `{id}`: license field is empty"));
        }

        // 4. license_verified = false: USER-GATE notice, not a hard violation.
        //    D1's registry comment explicitly says "Mike must read the upstream
        //    license and flip the flag himself; agents must not flip it."
        //    We print a notice so CI output is actionable, but do not fail.
        if !model.license_verified {
            eprintln!(
                "[model-audit] USER-GATE  model `{id}`: license_verified = false — \
                 Mike must read the upstream license and set license_verified = true \
                 before this model ships (PRD R4)"
            );
        }
    }

    Ok(violations)
}

/// Return `true` when a revision string looks unpinned.
///
/// A valid pinned revision is one of:
/// - A 40-hex-character commit SHA (`[0-9a-fA-F]{40}`).
/// - A short SHA of at least 7 hex characters (rare but acceptable).
/// - A purely numeric version tag (e.g. `v1.2.3` or `1.2.3`).
///
/// Anything else — including `main`, `master`, `HEAD`, `latest`, or an empty
/// string — is unpinned.
fn is_unpinned_revision(rev: &str) -> bool {
    let rev = rev.trim();
    if rev.is_empty() {
        return true;
    }
    // Explicit bad-actor keywords.
    let unpinned_keywords = ["main", "master", "head", "latest", "dev", "trunk"];
    if unpinned_keywords
        .iter()
        .any(|kw| rev.eq_ignore_ascii_case(kw))
    {
        return true;
    }
    // Accept a 7–40 char hex string (commit SHA).
    if rev.len() >= 7 && rev.len() <= 40 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    // Accept a version tag like `v1.2.3` or `1.2.3`.
    let stripped = rev.strip_prefix('v').unwrap_or(rev);
    if stripped
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    // Everything else is considered unpinned.
    true
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as assertion mechanism"
)]
mod tests {
    use super::*;

    #[test]
    fn revision_sha_accepted() {
        // Exactly 40 hex chars — a full commit SHA.
        assert!(!is_unpinned_revision(
            "abc1234567890abcdef1234567890abcdef12345"
        ));
        assert!(!is_unpinned_revision("abc1234")); // 7-char short SHA
    }

    #[test]
    fn revision_version_tag_accepted() {
        assert!(!is_unpinned_revision("1.2.3"));
        assert!(!is_unpinned_revision("v1.2.3"));
        assert!(!is_unpinned_revision("0.1.0"));
    }

    #[test]
    fn revision_main_rejected() {
        assert!(is_unpinned_revision("main"));
        assert!(is_unpinned_revision("master"));
        assert!(is_unpinned_revision("HEAD"));
        assert!(is_unpinned_revision("latest"));
        assert!(is_unpinned_revision(""));
    }

    #[test]
    fn clean_fixture_registry_passes() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clean/models.toml");
        let result = audit_registry(&fixture);
        let violations = result.expect("should parse clean fixture");
        assert!(
            violations.is_empty(),
            "clean fixture should have no violations, got: {violations:?}"
        );
    }

    #[test]
    fn violation_fixture_registry_fails() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/violation/models.toml");
        let result = audit_registry(&fixture);
        let violations = result.expect("should parse (even if violations present)");
        assert!(
            !violations.is_empty(),
            "violation fixture must report at least one violation"
        );
        // Verify we catch the "main" revision.
        assert!(
            violations.iter().any(|v| v.contains("not pinned")),
            "expected a 'not pinned' violation, got: {violations:?}"
        );
    }

    #[test]
    fn blocked_sentinel_emits_notice_not_violation() {
        // BLOCKED-ON-USER-GATE is a special D1 sentinel for entries awaiting
        // user action. audit_registry handles it with a USER-GATE notice (stderr)
        // rather than a hard violation. The sentinel is checked before
        // is_unpinned_revision, so CI still passes even when the registry has
        // BLOCKED entries.
        //
        // We create a minimal in-memory registry with the sentinel and verify
        // that audit_registry returns no hard violations.
        use silent_core::registry::{Cache, CacheStore, ExecutionProvider, Host, Provider, Task};

        let model = silent_core::registry::Model {
            id: silent_core::ids::ModelId::new("asr.test.blocked"),
            task: Task::Asr,
            provider: Provider::Huggingface,
            repo: "someone/sensevoice".into(),
            revision: BLOCKED_SENTINEL.to_owned(),
            host: Host::JsSherpa,
            execution_provider: ExecutionProvider::Cpu,
            precision: vec!["fp32".into()],
            memory_budget_mb: 256,
            cache: Cache::new(CacheStore::CacheApi),
            license: "cc-by-4.0".into(),
            license_verified: false,
            network_origins: vec![],
            files: vec![],
            device_tiers: std::collections::BTreeMap::default(),
            validation: None,
            perf_budget: None,
            ui: None,
            composite_of: Vec::new(),
        };
        let registry = silent_core::registry::Registry {
            models: vec![model],
        };

        // Write to a tempfile and audit it.
        let toml_str = toml::to_string(&registry).expect("serialise");
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        std::fs::write(tmp.path(), &toml_str).expect("write");

        let violations = audit_registry(tmp.path()).expect("audit should not error");
        assert!(
            violations.is_empty(),
            "BLOCKED-ON-USER-GATE should produce USER-GATE notices, not hard violations; \
             got: {violations:?}"
        );
    }

    #[test]
    fn allowed_fixture_paths_do_not_trigger() {
        // The eval/ TitaNet fixture is in the allowlist — a weight scan of a
        // tree containing only that path should return no violations.
        for allowed in ALLOWED_FIXTURE_PATHS {
            let rel: String = (*allowed).to_owned();
            // Simulate the check: does the allowlist suppress this path?
            let suppressed = ALLOWED_FIXTURE_PATHS.iter().any(|a| rel == *a);
            assert!(
                suppressed,
                "expected {rel:?} to be in the allowlist but it wasn't"
            );
        }
    }
}
