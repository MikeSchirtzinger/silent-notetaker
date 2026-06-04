//! Manifest validation (PRD R7 acceptance).
//!
//! Two layers protect the boundary:
//!
//! 1. **Decode-time (serde).** A manifest that names a capability outside the
//!    vocabulary — `transcript.raw_audio`, `embeddings`, `mel`, a wildcard
//!    origin — fails to *decode* into a [`Manifest`], because those tokens are
//!    not variants of the capability enums and origins route through
//!    [`crate::capability::NetworkGrant::parse`]. This is the type-level floor:
//!    "an extension requesting raw audio is rejected at manifest validation" is
//!    satisfied before this module even runs, because such a request cannot be
//!    represented.
//!
//! 2. **Policy-time (this module).** Even a well-typed manifest can be
//!    nonsensical or abusive: an empty entrypoint, duplicate capability tokens,
//!    a UI message a panel-less manifest could never honour, or an oversize
//!    document. [`validate`] turns each into a precise [`ManifestError`].
//!
//! [`parse_and_validate`] runs both layers and maps a serde decode failure into
//! a [`ManifestError::Decode`] so callers get one error type. The adversarial
//! manifests in `tests/` exercise every rejection path.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::capability::{DataCapability, UiCapability};
use crate::manifest::Manifest;

/// The maximum size, in bytes, of a `manifest.json` document this crate will
/// parse. A manifest is a small declaration; anything larger is treated as
/// hostile (a decompression bomb, a padded blob) and rejected before serde even
/// runs, so the parser is never handed an unbounded input.
pub const MAX_MANIFEST_BYTES: usize = 16 * 1024;

/// The maximum number of network grants a single manifest may request. A
/// privacy-first extension talks to a handful of origins; an unbounded list is
/// a smell and a denial-of-service vector against the per-extension CSP.
pub const MAX_NETWORK_GRANTS: usize = 16;

/// Parse a raw `manifest.json` byte slice and fully validate it.
///
/// This is the single entry point a host should call. It enforces the size
/// bound, decodes with `serde_json`, then runs [`validate`].
///
/// # Errors
///
/// Returns a [`ManifestError`]:
/// - [`ManifestError::TooLarge`] if `raw` exceeds [`MAX_MANIFEST_BYTES`].
/// - [`ManifestError::Decode`] if the JSON is malformed *or* names a capability
///   outside the vocabulary / a malformed origin (the type-level rejection).
/// - any [`ManifestError`] from [`validate`] for a well-typed but invalid
///   manifest.
pub fn parse_and_validate(raw: &[u8]) -> Result<Manifest, ManifestError> {
    if raw.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge {
            len: raw.len(),
            max: MAX_MANIFEST_BYTES,
        });
    }

    let manifest: Manifest = serde_json::from_slice(raw).map_err(|e| ManifestError::Decode {
        reason: e.to_string(),
    })?;

    validate(&manifest)?;
    Ok(manifest)
}

/// Validate an already-decoded manifest against the policy rules.
///
/// Decode-time guarantees (vocabulary membership, origin well-formedness) are
/// assumed already met — a [`Manifest`] cannot exist otherwise. This checks the
/// remaining policy invariants.
///
/// # Errors
///
/// Returns the first [`ManifestError`] encountered. Order is deterministic so
/// tests can assert on the exact error.
pub fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    // Entrypoint must be a non-empty, relative path with no traversal or
    // absolute/scheme prefix — it is loaded as a Worker URL by the host.
    let entry = manifest.entrypoint.trim();
    if entry.is_empty() {
        return Err(ManifestError::EmptyEntrypoint);
    }
    if entry.starts_with('/') || entry.contains("://") {
        return Err(ManifestError::AbsoluteEntrypoint {
            entrypoint: manifest.entrypoint.clone(),
        });
    }
    if entry.split('/').any(|seg| seg == "..") {
        return Err(ManifestError::EntrypointTraversal {
            entrypoint: manifest.entrypoint.clone(),
        });
    }

    // Duplicate data capabilities.
    if let Some(dup) = first_duplicate_data(&manifest.capabilities.data) {
        return Err(ManifestError::DuplicateDataCapability {
            capability: dup.token().to_owned(),
        });
    }

    // Duplicate UI capabilities.
    if let Some(dup) = first_duplicate_ui(&manifest.capabilities.ui) {
        return Err(ManifestError::DuplicateUiCapability {
            capability: dup.token().to_owned(),
        });
    }

    // Duplicate network grants (string-equal origins).
    if let Some(dup) = first_duplicate_grant(manifest) {
        return Err(ManifestError::DuplicateNetworkGrant { origin: dup });
    }

    // Too many network grants.
    if manifest.capabilities.network.len() > MAX_NETWORK_GRANTS {
        return Err(ManifestError::TooManyNetworkGrants {
            count: manifest.capabilities.network.len(),
            max: MAX_NETWORK_GRANTS,
        });
    }

    Ok(())
}

fn first_duplicate_data(items: &[DataCapability]) -> Option<DataCapability> {
    let mut seen = BTreeSet::new();
    items.iter().copied().find(|&c| !seen.insert(c))
}

fn first_duplicate_ui(items: &[UiCapability]) -> Option<UiCapability> {
    let mut seen = BTreeSet::new();
    items.iter().copied().find(|&c| !seen.insert(c))
}

fn first_duplicate_grant(manifest: &Manifest) -> Option<String> {
    let mut seen = BTreeSet::new();
    manifest
        .capabilities
        .network
        .iter()
        .find(|g| !seen.insert(g.as_str()))
        .map(|g| g.as_str().to_owned())
}

/// Why a manifest is rejected.
///
/// `#[non_exhaustive]`; callers must include a wildcard arm. Each variant is a
/// precise, self-contained message suitable for an install error or a log line.
/// Forbidden-capability and wildcard-origin requests do **not** appear here as
/// runtime variants — they are rejected one layer earlier, at decode, and
/// surface as [`ManifestError::Decode`] carrying serde's message (which names
/// the unknown token / bad origin).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ManifestError {
    /// The raw document exceeds [`MAX_MANIFEST_BYTES`].
    #[error("manifest is {len} bytes; the maximum is {max}")]
    TooLarge {
        /// Actual size.
        len: usize,
        /// Allowed maximum.
        max: usize,
    },

    /// `serde_json` could not decode the document into a [`Manifest`]. This is
    /// also the path a forbidden capability (raw audio, embeddings, mel, model
    /// activations) or a wildcard / malformed origin takes: the token is not in
    /// the vocabulary, so decode fails here with serde's reason.
    #[error("manifest could not be decoded: {reason}")]
    Decode {
        /// The serde error message (names the offending field / token).
        reason: String,
    },

    /// The `entrypoint` is empty or whitespace.
    #[error("manifest entrypoint must not be empty")]
    EmptyEntrypoint,

    /// The `entrypoint` is absolute or carries a URL scheme.
    #[error("manifest entrypoint `{entrypoint}` must be a relative path, not absolute or a URL")]
    AbsoluteEntrypoint {
        /// The offending entrypoint.
        entrypoint: String,
    },

    /// The `entrypoint` contains a `..` path-traversal segment.
    #[error("manifest entrypoint `{entrypoint}` must not contain `..`")]
    EntrypointTraversal {
        /// The offending entrypoint.
        entrypoint: String,
    },

    /// A data capability is listed more than once.
    #[error("manifest lists data capability `{capability}` more than once")]
    DuplicateDataCapability {
        /// The duplicated capability token.
        capability: String,
    },

    /// A UI capability is listed more than once.
    #[error("manifest lists UI capability `{capability}` more than once")]
    DuplicateUiCapability {
        /// The duplicated capability token.
        capability: String,
    },

    /// A network origin is listed more than once.
    #[error("manifest lists network grant `{origin}` more than once")]
    DuplicateNetworkGrant {
        /// The duplicated origin.
        origin: String,
    },

    /// More than [`MAX_NETWORK_GRANTS`] network origins requested.
    #[error("manifest requests {count} network grants; the maximum is {max}")]
    TooManyNetworkGrants {
        /// Actual count.
        count: usize,
        /// Allowed maximum.
        max: usize,
    },
}
