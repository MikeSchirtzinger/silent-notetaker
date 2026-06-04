//! `xtask deploy-gate` — gate a deploy bundle before pushing to Cloudflare Pages.
//!
//! Replaces the unchecked `deploy-cloudflare.sh` flow (PRD R6). Three checks:
//!
//! 1. **No model weight files** — any `.onnx`, `.gguf`, `.safetensors`, `.bin`,
//!    `.pt`, or `.pth` file in the bundle fails the gate.
//!
//! 2. **No file > 25 MB** — Cloudflare Pages rejects files over 25 MB (PRD R6).
//!    This check catches weights that slipped through check 1 (e.g. a `.npz`)
//!    and any other accidentally large artifact.
//!
//! 3. **`_headers` is fresh** — delegates to `gen-headers --check` semantics
//!    to verify the bundled `_headers` matches what the current registry + static
//!    invariants would generate.

use anyhow::{Context, Result, bail};
use silent_core::registry::Registry;
use std::path::{Path, PathBuf};

/// Arguments for `xtask deploy-gate`.
#[derive(clap::Args, Debug)]
pub struct DeployGateArgs {
    /// The bundle directory to inspect (e.g. `dist/`).
    pub bundle_dir: PathBuf,

    /// Path to the registry TOML. Defaults to `<repo_root>/registry/models.toml`.
    #[arg(long)]
    pub registry: Option<PathBuf>,

    /// Skip the `_headers` freshness check (e.g. when testing without a
    /// registry; use with care).
    #[arg(long)]
    pub skip_headers_check: bool,
}

/// File extensions that identify model weight files (same set as model-audit).
const WEIGHT_EXTENSIONS: &[&str] = &["onnx", "gguf", "safetensors", "bin", "pt", "pth"];

/// Cloudflare Pages per-file size limit in bytes (25 MB).
const CF_SIZE_LIMIT_BYTES: u64 = 25 * 1024 * 1024;

/// Run `xtask deploy-gate`.
pub fn run(args: DeployGateArgs) -> Result<()> {
    let bundle_dir = &args.bundle_dir;
    if !bundle_dir.is_dir() {
        bail!(
            "bundle directory does not exist or is not a directory: {}",
            bundle_dir.display()
        );
    }
    eprintln!("[deploy-gate] checking bundle: {}", bundle_dir.display());

    let mut violations: Vec<String> = Vec::new();

    // -----------------------------------------------------------------------
    // 1. Scan for weight files and oversized files.
    // -----------------------------------------------------------------------
    scan_bundle(bundle_dir, &mut violations)?;

    // -----------------------------------------------------------------------
    // 2. Check _headers freshness.
    // -----------------------------------------------------------------------
    if !args.skip_headers_check {
        let headers_path = bundle_dir.join("_headers");
        let repo_root = resolve_repo_root()?;
        let registry_path = args
            .registry
            .unwrap_or_else(|| repo_root.join("registry").join("models.toml"));

        if let Err(e) = check_headers_freshness(&headers_path, &registry_path) {
            violations.push(format!("_headers freshness check failed: {e}"));
        }
    }

    // -----------------------------------------------------------------------
    // Report.
    // -----------------------------------------------------------------------
    if violations.is_empty() {
        eprintln!("[deploy-gate] PASS — bundle is clean.");
        Ok(())
    } else {
        eprintln!("[deploy-gate] FAIL — {} violation(s):", violations.len());
        for v in &violations {
            eprintln!("  {v}");
        }
        bail!(
            "deploy-gate found {} violation(s); see above",
            violations.len()
        )
    }
}

/// Walk all files in the bundle directory and collect weight / size violations.
fn scan_bundle(bundle_dir: &Path, violations: &mut Vec<String>) -> Result<()> {
    for entry in walkdir::WalkDir::new(bundle_dir).follow_links(false) {
        let entry = entry.context("walkdir error")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(bundle_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();

        // Check weight extension.
        let has_weight_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| {
                WEIGHT_EXTENSIONS
                    .iter()
                    .any(|w| ext.eq_ignore_ascii_case(w))
            });

        if has_weight_ext {
            violations.push(format!(
                "model weight file in deploy bundle (extension violation): {rel}"
            ));
        }

        // Check file size. The u64→f64 cast for the human-readable size message
        // is intentional (we only need ~1 decimal place; no precision matters here).
        #[allow(clippy::cast_precision_loss)]
        if let Ok(meta) = std::fs::metadata(path)
            && meta.len() > CF_SIZE_LIMIT_BYTES
        {
            violations.push(format!(
                "file exceeds Cloudflare Pages 25 MB limit: {rel} ({:.1} MB)",
                meta.len() as f64 / 1024.0 / 1024.0,
            ));
        }
    }
    Ok(())
}

/// Check that the `_headers` file in the bundle matches the generated output.
///
/// Reuses the `gen_headers` logic for consistency.
fn check_headers_freshness(headers_path: &Path, registry_path: &Path) -> Result<()> {
    if !headers_path.exists() {
        bail!(
            "_headers file not found in bundle at {} \
             (add it with: cargo xtask gen-headers --out {})",
            headers_path.display(),
            headers_path.display()
        );
    }

    let registry = if registry_path.exists() {
        let content = std::fs::read_to_string(registry_path)
            .with_context(|| format!("reading registry: {}", registry_path.display()))?;
        toml::from_str::<Registry>(&content)
            .with_context(|| format!("parsing registry TOML: {}", registry_path.display()))?
    } else {
        eprintln!(
            "[deploy-gate] registry not found at {} — checking headers against \
             static invariants only",
            registry_path.display()
        );
        Registry::default()
    };

    let generated = crate::gen_headers::generate_headers(&registry);
    let on_disk = std::fs::read_to_string(headers_path)
        .with_context(|| format!("reading {}", headers_path.display()))?;

    if on_disk == generated {
        eprintln!("[deploy-gate] _headers: fresh.");
        Ok(())
    } else {
        bail!(
            "_headers is stale — regenerate with: \
             cargo xtask gen-headers --out {}",
            headers_path.display()
        )
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
    clippy::cast_possible_truncation,
    reason = "tests use unwrap/expect as assertion mechanism; \
              cast_possible_truncation is acceptable for test data sizing on 64-bit targets"
)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a bundle directory with the given files.
    ///
    /// `files` maps relative path → content bytes.
    fn make_bundle(files: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, content) in files {
            let abs = dir.path().join(rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).expect("create_dir_all");
            }
            std::fs::write(&abs, content).expect("write file");
        }
        let path = dir.path().to_owned();
        (dir, path)
    }

    #[test]
    fn clean_bundle_passes() {
        let (_dir, bundle) = make_bundle(&[
            ("index.html", b"<html></html>".to_vec()),
            ("app.js", b"console.log('hi');".to_vec()),
        ]);
        let mut violations = Vec::new();
        scan_bundle(&bundle, &mut violations).expect("scan");
        assert!(
            violations.is_empty(),
            "clean bundle should have no violations: {violations:?}"
        );
    }

    #[test]
    fn onnx_weight_detected() {
        let (_dir, bundle) = make_bundle(&[
            ("index.html", b"<html></html>".to_vec()),
            ("model.onnx", b"fake onnx data".to_vec()),
        ]);
        let mut violations = Vec::new();
        scan_bundle(&bundle, &mut violations).expect("scan");
        assert!(
            violations.iter().any(|v| v.contains("model weight file")),
            "expected weight violation, got: {violations:?}"
        );
    }

    #[test]
    fn oversized_file_detected() {
        // Create a file that is exactly CF_SIZE_LIMIT_BYTES + 1 bytes.
        let big_data = vec![0u8; CF_SIZE_LIMIT_BYTES as usize + 1];
        let (_dir, bundle) = make_bundle(&[
            ("index.html", b"<html></html>".to_vec()),
            ("huge.wasm", big_data),
        ]);
        let mut violations = Vec::new();
        scan_bundle(&bundle, &mut violations).expect("scan");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("exceeds Cloudflare Pages")),
            "expected size violation, got: {violations:?}"
        );
    }

    #[test]
    fn safetensors_detected() {
        let (_dir, bundle) = make_bundle(&[("weights.safetensors", b"fake".to_vec())]);
        let mut violations = Vec::new();
        scan_bundle(&bundle, &mut violations).expect("scan");
        assert!(
            violations.iter().any(|v| v.contains("model weight file")),
            "expected weight violation for safetensors: {violations:?}"
        );
    }

    #[test]
    fn fresh_headers_passes() {
        use silent_core::registry::Registry;
        let registry = Registry::default();
        let generated = crate::gen_headers::generate_headers(&registry);

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(generated.as_bytes()).expect("write");

        // Build a bundle with this _headers file.
        let bundle_dir = tempfile::tempdir().expect("tempdir");
        let headers_path = bundle_dir.path().join("_headers");
        std::fs::write(&headers_path, &generated).expect("write headers");

        // check_headers_freshness should pass.
        let result = check_headers_freshness(
            &headers_path,
            &PathBuf::from("/nonexistent/registry/models.toml"),
        );
        assert!(result.is_ok(), "fresh headers should pass: {result:?}");
    }

    #[test]
    fn stale_headers_fails() {
        let bundle_dir = tempfile::tempdir().expect("tempdir");
        let headers_path = bundle_dir.path().join("_headers");
        std::fs::write(&headers_path, "stale headers content").expect("write");

        let result = check_headers_freshness(
            &headers_path,
            &PathBuf::from("/nonexistent/registry/models.toml"),
        );
        assert!(result.is_err(), "stale headers should fail: {result:?}");
    }

    #[test]
    fn full_gate_run_clean_bundle_passes() {
        use silent_core::registry::Registry;
        // Build a clean bundle: index.html + fresh _headers.
        let registry = Registry::default();
        let headers_content = crate::gen_headers::generate_headers(&registry);

        let bundle_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(bundle_dir.path().join("index.html"), "<html></html>").expect("write");
        std::fs::write(bundle_dir.path().join("_headers"), &headers_content).expect("write");

        let args = DeployGateArgs {
            bundle_dir: bundle_dir.path().to_owned(),
            registry: None,
            skip_headers_check: true, // registry path won't exist; skip to test bundle scan
        };
        run(args).expect("clean bundle should pass deploy gate");
    }

    #[test]
    fn full_gate_run_weight_violation() {
        let bundle_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(bundle_dir.path().join("model.onnx"), b"fake").expect("write");

        let args = DeployGateArgs {
            bundle_dir: bundle_dir.path().to_owned(),
            registry: None,
            skip_headers_check: true,
        };
        let result = run(args);
        assert!(result.is_err(), "bundle with .onnx should fail: {result:?}");
    }

    #[test]
    fn full_gate_run_oversized_violation() {
        let bundle_dir = tempfile::tempdir().expect("tempdir");
        let big = vec![0u8; CF_SIZE_LIMIT_BYTES as usize + 1];
        std::fs::write(bundle_dir.path().join("huge.bin"), &big).expect("write");

        // Note: huge.bin also has a weight extension (.bin) so both violations fire.
        let args = DeployGateArgs {
            bundle_dir: bundle_dir.path().to_owned(),
            registry: None,
            skip_headers_check: true,
        };
        let result = run(args);
        assert!(result.is_err(), "oversized file should fail: {result:?}");
    }
}
