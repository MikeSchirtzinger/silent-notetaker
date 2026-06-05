//! The capability vocabulary (PRD R7).
//!
//! Capabilities are the *only* thing an extension may request. The vocabulary
//! is a closed, finite set of enums. **This is the type-level privacy floor**:
//! raw audio samples, raw voice embeddings, mel-spectrogram tensors, and
//! intermediate model activations are **not** variants of any capability enum,
//! so an extension literally cannot name them in a manifest. There is no parse
//! path, no `serde` alias, and no constructor that yields them — the privacy
//! guarantee is enforced by the *absence* of a type, not by a runtime check
//! that could be forgotten (PRD R3/R7, `docs/EXTENSIONS.md` §1.1).
//!
//! The three capability axes mirror the manifest's `capabilities` object:
//!
//! - [`DataCapability`] — what meeting data the extension may read.
//! - [`UiCapability`]   — what UI surface the extension may drive.
//! - [`NetworkGrant`]   — which origins the extension may reach (origin-scoped,
//!   never a wildcard).
//!
//! # Why an enum and not a string
//!
//! `docs/EXTENSIONS.md` describes capability *tokens* (`"transcript.text"`).
//! Those tokens are the wire form; this module is the parsed form. Modelling
//! them as enums means an *unknown* token (a typo, or a forbidden request like
//! `"audio.raw"`) fails to deserialize into a [`DataCapability`] at all, which
//! is exactly the behaviour [`crate::validation`] turns into a precise,
//! user-facing error. A free-form `String` would silently accept anything.

use serde::{Deserialize, Serialize};

/// A meeting-data capability: a single readable data surface.
///
/// `#[non_exhaustive]` so new *safe* data surfaces can be added without a
/// breaking change. Crucially, the enum contains **no** variant for raw audio,
/// embeddings, mel tensors, or model activations — those are not part of the
/// extension API surface at any version (PRD R7 acceptance: "an extension
/// requesting raw audio is rejected at manifest validation"; here it is
/// rejected one step earlier — it cannot even be represented).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DataCapability {
    /// The running transcript as plain text segments, timestamped
    /// (`transcript.text`).
    #[serde(rename = "transcript.text")]
    TranscriptText,

    /// Segments with speaker label, start/end timestamps, and confidence
    /// (`transcript.segments`).
    #[serde(rename = "transcript.segments")]
    TranscriptSegments,

    /// Extracted decisions, text only (`notes.decisions`).
    #[serde(rename = "notes.decisions")]
    NotesDecisions,

    /// Extracted action items (`notes.actions`).
    #[serde(rename = "notes.actions")]
    NotesActions,

    /// Extracted key points (`notes.keypoints`).
    #[serde(rename = "notes.keypoints")]
    NotesKeypoints,

    /// Open questions surfaced during the meeting (`notes.questions`).
    #[serde(rename = "notes.questions")]
    NotesQuestions,

    /// The current `S1 -> "Alice"` rename map (`speaker.labels`).
    #[serde(rename = "speaker.labels")]
    SpeakerLabels,

    /// Title, start time, duration — no audio, no embeddings
    /// (`meeting.metadata`).
    #[serde(rename = "meeting.metadata")]
    MeetingMetadata,
}

impl DataCapability {
    /// Every data capability in the vocabulary, in declaration order.
    ///
    /// Used by validation and by UIs that render the install-time consent
    /// screen. The list is finite and exhaustive by construction.
    pub const ALL: &'static [DataCapability] = &[
        DataCapability::TranscriptText,
        DataCapability::TranscriptSegments,
        DataCapability::NotesDecisions,
        DataCapability::NotesActions,
        DataCapability::NotesKeypoints,
        DataCapability::NotesQuestions,
        DataCapability::SpeakerLabels,
        DataCapability::MeetingMetadata,
    ];

    /// The wire token for this capability (the `capabilities.data` string).
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            DataCapability::TranscriptText => "transcript.text",
            DataCapability::TranscriptSegments => "transcript.segments",
            DataCapability::NotesDecisions => "notes.decisions",
            DataCapability::NotesActions => "notes.actions",
            DataCapability::NotesKeypoints => "notes.keypoints",
            DataCapability::NotesQuestions => "notes.questions",
            DataCapability::SpeakerLabels => "speaker.labels",
            DataCapability::MeetingMetadata => "meeting.metadata",
        }
    }
}

/// A UI-surface capability.
///
/// `#[non_exhaustive]` for future safe surfaces. Extensions never get free-form
/// DOM injection — a panel is a host-controlled sandboxed iframe slot
/// (`docs/EXTENSIONS.md` §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum UiCapability {
    /// Render a side panel inside a sandboxed iframe slot (`panel`).
    Panel,

    /// Post a short, text-only in-app toast (`notification`).
    Notification,
}

impl UiCapability {
    /// Every UI capability in the vocabulary, in declaration order.
    pub const ALL: &'static [UiCapability] = &[UiCapability::Panel, UiCapability::Notification];

    /// The wire token for this capability (the `capabilities.ui` string).
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            UiCapability::Panel => "panel",
            UiCapability::Notification => "notification",
        }
    }
}

/// An origin-scoped network grant.
///
/// Network is denied by default. A grant is a single full origin
/// (`https://api.notion.com`) — never a wildcard, never a bare host, never a
/// path. The grant is shown to the user at install and is revocable
/// (`docs/EXTENSIONS.md` §1.3). The contained string is validated by
/// [`NetworkGrant::parse`]; the only way to obtain a [`NetworkGrant`] is through that
/// constructor (or `serde`, which routes through the same validation), so an
/// in-memory [`NetworkGrant`] is always a well-formed, wildcard-free origin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, as = "String"))]
#[serde(transparent)]
pub struct NetworkGrant(String);

impl NetworkGrant {
    /// Parse a string as a strict origin: `scheme://host[:port]`, no path,
    /// query, fragment, userinfo, or wildcard.
    ///
    /// Rules (deliberately strict — the CSP `connect-src` floor depends on
    /// every grant being an exact origin):
    ///
    /// - scheme must be `https` (or `http` only for `localhost` /
    ///   `127.0.0.1`, to keep the local Claude bridge developable — see PRD
    ///   "the local Claude bridge"); `http` to any other host is rejected.
    /// - host must be present and contain no `*` (no wildcard origins).
    /// - no path component: the input must be exactly the origin, optionally
    ///   with a trailing `:port`. A trailing `/` is *not* accepted, to keep the
    ///   stored form canonical and byte-comparable against a CSP token.
    ///
    /// # Errors
    ///
    /// Returns a [`NetworkGrantError`] describing precisely why the string is not a
    /// valid origin grant. The message is suitable for surfacing at install.
    pub fn parse(raw: impl Into<String>) -> Result<NetworkGrant, NetworkGrantError> {
        let raw = raw.into();

        let (scheme, rest) =
            raw.split_once("://")
                .ok_or_else(|| NetworkGrantError::MissingScheme {
                    origin: raw.clone(),
                })?;

        let scheme_lower = scheme.to_ascii_lowercase();
        // `rest` is `host[:port][/...]`. Reject anything past the authority.
        if let Some(idx) = rest.find(['/', '?', '#']) {
            // A bare trailing slash with nothing after it is still a path
            // component; reject it to keep the canonical form slash-free.
            let trailing = &rest[idx..];
            return Err(NetworkGrantError::HasPath {
                origin: raw.clone(),
                trailing: trailing.to_owned(),
            });
        }

        // Userinfo (`user@host`) is not part of an origin and must be rejected.
        if rest.contains('@') {
            return Err(NetworkGrantError::HasUserinfo {
                origin: raw.clone(),
            });
        }

        // Wildcards are forbidden everywhere in the authority.
        if rest.contains('*') {
            return Err(NetworkGrantError::Wildcard {
                origin: raw.clone(),
            });
        }

        let host = match rest.split_once(':') {
            Some((host, port)) => {
                if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(NetworkGrantError::BadPort {
                        origin: raw.clone(),
                        port: port.to_owned(),
                    });
                }
                host
            }
            None => rest,
        };

        if host.is_empty() {
            return Err(NetworkGrantError::MissingHost {
                origin: raw.clone(),
            });
        }

        let is_loopback =
            host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]";

        match scheme_lower.as_str() {
            "https" => {}
            "http" if is_loopback => {}
            "http" => {
                return Err(NetworkGrantError::InsecureScheme {
                    origin: raw.clone(),
                });
            }
            _ => {
                return Err(NetworkGrantError::UnsupportedScheme {
                    origin: raw.clone(),
                    scheme: scheme.to_owned(),
                });
            }
        }

        Ok(NetworkGrant(raw))
    }

    /// The canonical origin string (exactly what goes into `connect-src`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NetworkGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Deserialization routes through `parse`, so an `NetworkGrant` decoded from a
// manifest is validated identically to one built in Rust. There is no way to
// construct an unvalidated `NetworkGrant`.
impl<'de> Deserialize<'de> for NetworkGrant {
    fn deserialize<D>(deserializer: D) -> Result<NetworkGrant, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        NetworkGrant::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Why a string is not a valid origin grant.
///
/// `#[non_exhaustive]`; callers must include a wildcard arm. Each variant
/// carries the offending origin so the message is self-contained at the install
/// consent screen and in logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NetworkGrantError {
    /// No `scheme://` prefix.
    #[error("network grant `{origin}` is not an origin: missing `scheme://`")]
    MissingScheme {
        /// The rejected grant string.
        origin: String,
    },

    /// Scheme is not `https` and the host is not loopback.
    #[error("network grant `{origin}` must use https (http is allowed only for localhost)")]
    InsecureScheme {
        /// The rejected grant string.
        origin: String,
    },

    /// Scheme is neither `http` nor `https`.
    #[error("network grant `{origin}` uses unsupported scheme `{scheme}` (only http/https)")]
    UnsupportedScheme {
        /// The rejected grant string.
        origin: String,
        /// The offending scheme.
        scheme: String,
    },

    /// The authority is empty.
    #[error("network grant `{origin}` has no host")]
    MissingHost {
        /// The rejected grant string.
        origin: String,
    },

    /// The grant contains a `*` wildcard.
    #[error("network grant `{origin}` contains a wildcard; grants must be exact origins")]
    Wildcard {
        /// The rejected grant string.
        origin: String,
    },

    /// The grant carries a path / query / fragment beyond the origin.
    #[error("network grant `{origin}` must be an origin only; remove the trailing `{trailing}`")]
    HasPath {
        /// The rejected grant string.
        origin: String,
        /// The offending suffix (path/query/fragment).
        trailing: String,
    },

    /// The grant carries `user@host` userinfo.
    #[error("network grant `{origin}` must not contain userinfo (`user@host`)")]
    HasUserinfo {
        /// The rejected grant string.
        origin: String,
    },

    /// The port is empty or non-numeric.
    #[error("network grant `{origin}` has an invalid port `{port}`")]
    BadPort {
        /// The rejected grant string.
        origin: String,
        /// The offending port substring.
        port: String,
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
    fn vocabulary_token_round_trips() {
        // Every data capability's token serializes to itself and back.
        for &cap in DataCapability::ALL {
            let json = serde_json::to_string(&cap).unwrap();
            assert_eq!(json, format!("\"{}\"", cap.token()));
            let back: DataCapability = serde_json::from_str(&json).unwrap();
            assert_eq!(cap, back);
        }
        for &cap in UiCapability::ALL {
            let json = serde_json::to_string(&cap).unwrap();
            assert_eq!(json, format!("\"{}\"", cap.token()));
        }
    }

    #[test]
    fn good_origins_parse_canonically() {
        for o in [
            "https://api.notion.com",
            "https://api.notion.com:8443",
            "https://sub.domain.example.com",
            "http://localhost:8765",
            "http://127.0.0.1",
        ] {
            let g = NetworkGrant::parse(o).unwrap();
            assert_eq!(g.as_str(), o, "origin must be stored canonically");
        }
    }

    #[test]
    fn wildcard_origin_is_rejected() {
        assert!(matches!(
            NetworkGrant::parse("https://*.notion.com"),
            Err(NetworkGrantError::Wildcard { .. })
        ));
    }

    #[test]
    fn path_query_fragment_are_rejected() {
        assert!(matches!(
            NetworkGrant::parse("https://a.com/v1"),
            Err(NetworkGrantError::HasPath { .. })
        ));
        assert!(matches!(
            NetworkGrant::parse("https://a.com/"),
            Err(NetworkGrantError::HasPath { .. })
        ));
        assert!(matches!(
            NetworkGrant::parse("https://a.com?x=1"),
            Err(NetworkGrantError::HasPath { .. })
        ));
        assert!(matches!(
            NetworkGrant::parse("https://a.com#frag"),
            Err(NetworkGrantError::HasPath { .. })
        ));
    }

    #[test]
    fn http_to_non_loopback_is_rejected() {
        assert!(matches!(
            NetworkGrant::parse("http://evil.com"),
            Err(NetworkGrantError::InsecureScheme { .. })
        ));
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        assert!(matches!(
            NetworkGrant::parse("ftp://a.com"),
            Err(NetworkGrantError::UnsupportedScheme { .. })
        ));
        assert!(matches!(
            NetworkGrant::parse("a.com"),
            Err(NetworkGrantError::MissingScheme { .. })
        ));
    }

    #[test]
    fn userinfo_and_bad_port_are_rejected() {
        assert!(matches!(
            NetworkGrant::parse("https://user@a.com"),
            Err(NetworkGrantError::HasUserinfo { .. })
        ));
        assert!(matches!(
            NetworkGrant::parse("https://a.com:abc"),
            Err(NetworkGrantError::BadPort { .. })
        ));
    }

    #[test]
    fn origin_deserialize_routes_through_parse() {
        // serde decoding is held to the same validation as the constructor.
        assert!(serde_json::from_str::<NetworkGrant>("\"https://*.x.com\"").is_err());
        let g: NetworkGrant = serde_json::from_str("\"https://api.notion.com\"").unwrap();
        assert_eq!(g.as_str(), "https://api.notion.com");
    }
}
