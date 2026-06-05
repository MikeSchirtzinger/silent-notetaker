//! The extension manifest schema (PRD R7, `docs/EXTENSIONS.md` §1).
//!
//! A [`Manifest`] is the parsed form of an extension's `manifest.json`. It is a
//! *declaration of intent only*: parsing a manifest never grants anything. The
//! capability fields are typed against the closed vocabulary in
//! [`crate::capability`], so a manifest can only ever name capabilities that
//! exist — there is no field, and no token, for raw audio / embeddings / mel
//! tensors / model activations.
//!
//! Parsing (`serde_json`) and *validating* ([`crate::validation`]) are separate
//! steps. `serde` enforces the *shape* (unknown capability tokens fail to
//! decode; see [`crate::capability::DataCapability`]); validation enforces the
//! *policy* (non-empty relative entrypoint, at most one of each capability,
//! bounded grant count, size bounds) and produces precise, user-facing errors.

use serde::{Deserialize, Serialize};

use crate::capability::{DataCapability, NetworkGrant, UiCapability};

/// A parsed `manifest.json`.
///
/// `#[serde(deny_unknown_fields)]` is deliberate: an unrecognised top-level key
/// is a malformed manifest, not a forward-compatible one. The privacy story
/// requires that a manifest cannot smuggle meaning through fields the host does
/// not understand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Machine-readable id; no spaces. Validated by [`ExtensionName`].
    pub name: ExtensionName,

    /// Human-facing name shown in the extension manager UI. Optional in the
    /// schema; the host falls back to `name`.
    #[serde(
        default,
        rename = "displayName",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<String>,

    /// Semver version string. Validated by [`Version`].
    pub version: Version,

    /// One-line description shown at install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Relative path to the worker entrypoint, for example `index.js`. Loaded
    /// in a Worker by the host (j2/j3); this crate only records and shape-checks
    /// it.
    pub entrypoint: String,

    /// The capabilities this extension declares it needs.
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// The `capabilities` object of a manifest.
///
/// Every axis defaults to empty. An extension that declares nothing gets
/// nothing — network is denied, no data crosses the boundary, no UI surface is
/// available.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Requested meeting-data surfaces. Unknown tokens fail to decode.
    #[serde(default)]
    pub data: Vec<DataCapability>,

    /// Requested UI surfaces.
    #[serde(default)]
    pub ui: Vec<UiCapability>,

    /// Requested network origins. Empty = network denied (the default).
    #[serde(default)]
    pub network: Vec<NetworkGrant>,
}

/// A validated extension name (the manifest `name`).
///
/// The wire form is a bare string. Construction goes through [`ExtensionName::parse`]
/// (or `serde`, which routes through it), so an in-memory [`ExtensionName`] is
/// always a legal id: 1..=64 chars, ASCII, lowercase letters / digits / `-` /
/// `_` / `.`, starting with a letter. This keeps a name safe to use as an
/// IndexedDB key, a CSP nonce label, and a postMessage `extensionId` without
/// escaping (`docs/EXTENSIONS.md` §1, §4 envelope).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, as = "String"))]
#[serde(transparent)]
pub struct ExtensionName(String);

/// Maximum extension-name length, in bytes/chars (ASCII, so equal).
pub const MAX_NAME_LEN: usize = 64;

impl ExtensionName {
    /// Parse and validate an extension name.
    ///
    /// # Errors
    ///
    /// Returns a [`NameError`] describing precisely why the name is invalid.
    pub fn parse(raw: impl Into<String>) -> Result<ExtensionName, NameError> {
        let raw = raw.into();

        if raw.is_empty() {
            return Err(NameError::Empty);
        }
        if raw.len() > MAX_NAME_LEN {
            return Err(NameError::TooLong {
                len: raw.len(),
                max: MAX_NAME_LEN,
            });
        }

        let mut chars = raw.chars();
        // First char must be an ASCII lowercase letter: keeps names from
        // colliding with numeric keys or starting with a separator.
        let first = chars.next().unwrap_or('\0');
        if !first.is_ascii_lowercase() {
            return Err(NameError::BadStart { ch: first });
        }
        for ch in raw.chars() {
            let ok = ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || ch == '-'
                || ch == '_'
                || ch == '.';
            if !ok {
                return Err(NameError::BadChar { ch });
            }
        }

        Ok(ExtensionName(raw))
    }

    /// Borrow the name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ExtensionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ExtensionName {
    fn deserialize<D>(deserializer: D) -> Result<ExtensionName, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        ExtensionName::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Why an extension name is invalid.
///
/// `#[non_exhaustive]`; callers must include a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NameError {
    /// The name is the empty string.
    #[error("extension name must not be empty")]
    Empty,

    /// The name exceeds [`MAX_NAME_LEN`].
    #[error("extension name is {len} chars; the maximum is {max}")]
    TooLong {
        /// Actual length.
        len: usize,
        /// Allowed maximum.
        max: usize,
    },

    /// The name does not start with an ASCII lowercase letter.
    #[error("extension name must start with a lowercase letter, found `{ch}`")]
    BadStart {
        /// The offending first character.
        ch: char,
    },

    /// The name contains a character outside `[a-z0-9._-]`.
    #[error("extension name contains illegal character `{ch}` (allowed: a-z 0-9 . _ -)")]
    BadChar {
        /// The offending character.
        ch: char,
    },
}

/// A validated semantic version (`MAJOR.MINOR.PATCH`).
///
/// A deliberately small, dependency-free semver: three non-negative integers.
/// Pre-release / build metadata (`-rc.1`, `+build`) is *rejected* rather than
/// parsed — extension versions are kept simple, and a strict parser keeps the
/// stored form canonical. The wire form is the bare `"1.2.3"` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, as = "String"))]
pub struct Version {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl Version {
    /// Construct a version from its three components.
    #[must_use]
    pub fn new(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    /// Parse a `MAJOR.MINOR.PATCH` string.
    ///
    /// # Errors
    ///
    /// Returns a [`VersionError`] for any deviation: wrong field count, an
    /// empty field, a non-numeric field, a leading-zero field, or a value that
    /// overflows `u32`.
    pub fn parse(raw: &str) -> Result<Version, VersionError> {
        let mut parts = raw.split('.');
        let major = Version::part(parts.next(), raw)?;
        let minor = Version::part(parts.next(), raw)?;
        let patch = Version::part(parts.next(), raw)?;
        if parts.next().is_some() {
            return Err(VersionError::Malformed {
                version: raw.to_owned(),
            });
        }
        Ok(Version {
            major,
            minor,
            patch,
        })
    }

    fn part(field: Option<&str>, raw: &str) -> Result<u32, VersionError> {
        let field = field.ok_or_else(|| VersionError::Malformed {
            version: raw.to_owned(),
        })?;
        if field.is_empty() {
            return Err(VersionError::Malformed {
                version: raw.to_owned(),
            });
        }
        // Reject leading zeros (`01`) to keep the form canonical, but allow a
        // lone `0`.
        if field.len() > 1 && field.starts_with('0') {
            return Err(VersionError::LeadingZero {
                version: raw.to_owned(),
                field: field.to_owned(),
            });
        }
        field.parse::<u32>().map_err(|_| VersionError::BadField {
            version: raw.to_owned(),
            field: field.to_owned(),
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Version, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Version::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Why a version string is invalid.
///
/// `#[non_exhaustive]`; callers must include a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum VersionError {
    /// Not exactly three dot-separated fields.
    #[error("version `{version}` must be MAJOR.MINOR.PATCH")]
    Malformed {
        /// The rejected version string.
        version: String,
    },

    /// A field is not a base-10 `u32`.
    #[error("version `{version}` has a non-numeric field `{field}`")]
    BadField {
        /// The rejected version string.
        version: String,
        /// The offending field.
        field: String,
    },

    /// A field has a non-canonical leading zero (`01`).
    #[error("version `{version}` field `{field}` has a leading zero")]
    LeadingZero {
        /// The rejected version string.
        version: String,
        /// The offending field.
        field: String,
    },
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the PRD lint \
              config allows this in tests"
)]
mod tests {
    use super::*;

    #[test]
    fn good_names_parse() {
        for n in ["notion-export", "a", "x.y.z", "with_underscore", "a1b2"] {
            assert_eq!(ExtensionName::parse(n).unwrap().as_str(), n);
        }
    }

    #[test]
    fn bad_names_are_rejected_precisely() {
        assert!(matches!(ExtensionName::parse(""), Err(NameError::Empty)));
        assert!(matches!(
            ExtensionName::parse("1abc"),
            Err(NameError::BadStart { ch: '1' })
        ));
        assert!(matches!(
            ExtensionName::parse("Upper"),
            Err(NameError::BadStart { ch: 'U' })
        ));
        assert!(matches!(
            ExtensionName::parse("has space"),
            Err(NameError::BadChar { ch: ' ' })
        ));
        assert!(matches!(
            ExtensionName::parse("has/slash"),
            Err(NameError::BadChar { ch: '/' })
        ));
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        assert!(matches!(
            ExtensionName::parse(too_long),
            Err(NameError::TooLong { .. })
        ));
    }

    #[test]
    fn good_versions_parse_and_order() {
        assert_eq!(Version::parse("1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(Version::parse("0.0.0").unwrap(), Version::new(0, 0, 0));
        assert!(Version::new(1, 0, 0) < Version::new(1, 0, 1));
        assert!(Version::new(1, 2, 0) < Version::new(2, 0, 0));
        assert_eq!(Version::new(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn bad_versions_are_rejected_precisely() {
        assert!(matches!(
            Version::parse("1.0"),
            Err(VersionError::Malformed { .. })
        ));
        assert!(matches!(
            Version::parse("1.0.0.0"),
            Err(VersionError::Malformed { .. })
        ));
        assert!(matches!(
            Version::parse("1.0.x"),
            Err(VersionError::BadField { .. })
        ));
        assert!(matches!(
            Version::parse("01.0.0"),
            Err(VersionError::LeadingZero { .. })
        ));
        assert!(matches!(
            Version::parse("1..0"),
            Err(VersionError::Malformed { .. })
        ));
    }

    #[test]
    fn version_serializes_as_a_bare_string() {
        let v = Version::new(3, 1, 4);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"3.1.4\"");
        let back: Version = serde_json::from_str("\"3.1.4\"").unwrap();
        assert_eq!(v, back);
    }
}
