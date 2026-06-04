//! Small shared value types used across the contracts.

use serde::{Deserialize, Serialize};

/// Stable local identifier for a model, for example `asr.nemotron.streaming_0_6b`.
///
/// This is a transparent newtype over `String`: it serializes as a bare string,
/// not as `{ "0": "..." }`. ts-rs cannot infer transparency from the
/// `#[serde(transparent)]` attribute and would otherwise emit a wrapper type, so
/// it is declared explicitly with `#[ts(as = "String")]` — the ts-rs 10 spelling
/// of the A3 spike's "transparent alongside serde(transparent)" guidance (the
/// `transparent` keyword the spike suggested is not a valid ts-rs 10 attribute).
/// The generated TypeScript is `type ModelId = string`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, as = "String"))]
#[serde(transparent)]
pub struct ModelId(pub String);

impl ModelId {
    /// Wrap a string as a [`ModelId`].
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the underlying id string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for ModelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// A half-open time span within a recording, in milliseconds from session start.
///
/// Attached to transcript events ([`crate::events::EngineEvent::Partial`],
/// [`crate::events::EngineEvent::Final`], …) so the UI can place and re-time
/// text. `end_ms >= start_ms` is an expected invariant but is not enforced by
/// the type (a degenerate range is preferable to a panic on the hot path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct TimeRange {
    /// Inclusive start, milliseconds from session start.
    pub start_ms: u64,
    /// Exclusive end, milliseconds from session start.
    pub end_ms: u64,
}

impl TimeRange {
    /// Construct a [`TimeRange`] from start/end milliseconds.
    #[must_use]
    pub fn new(start_ms: u64, end_ms: u64) -> Self {
        Self { start_ms, end_ms }
    }

    /// Duration of the range in milliseconds, saturating at zero for a
    /// degenerate (`end < start`) range.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}
