//! The versioned host <-> extension message protocol (PRD R7,
//! `docs/EXTENSIONS.md` §4, §5).
//!
//! These are the *only* types that cross the `postMessage` channel between the
//! host and a sandboxed extension worker. They are the wire contract; the
//! committed ts-rs bindings are how the (unchanged) JS host and any extension
//! author see them. Two design rules make the boundary trustworthy:
//!
//! 1. **Versioned.** Every message carries [`PROTOCOL_VERSION`] in its envelope.
//!    A host and an extension that disagree on the major version refuse to talk
//!    (the wiring layer enforces this; the type makes the version explicit and
//!    machine-checkable). The message enums are `#[non_exhaustive]` so new
//!    message types are an additive, minor-version change.
//!
//! 2. **No forbidden payloads exist.** Host-to-extension payloads carry
//!    transcript text, notes, speaker labels, and meeting metadata — and
//!    *nothing else*. There is no field, anywhere in this module, that carries
//!    raw audio, embeddings, mel tensors, or model activations. The privacy
//!    floor from [`crate::capability`] is mirrored here: the boundary cannot
//!    transmit what the vocabulary cannot name.
//!
//! The two directions are deliberately separate enums:
//!
//! - [`HostMessage`] — host -> extension (push: transcript / notes / lifecycle,
//!   plus the `export.response` reply).
//! - [`ExtensionMessage`] — extension -> host (requests: render a panel, post a
//!   notification, request an export snapshot).

use serde::{Deserialize, Serialize};

use crate::capability::DataCapability;
use crate::manifest::ExtensionName;

/// The protocol major version carried in every envelope.
///
/// Bumped only on a breaking change to the message shapes. A mismatched major
/// version is a hard refusal at the wiring layer; additive message types are a
/// minor concern handled by the `#[non_exhaustive]` enums and are not reflected
/// here.
pub const PROTOCOL_VERSION: u32 = 1;

/// The shared envelope around every message in either direction.
///
/// `T` is the direction-specific body ([`HostMessage`] or [`ExtensionMessage`]).
/// The envelope binds a message to a protocol version and to the extension it
/// concerns, so the receiver can reject a version mismatch and route by id
/// before inspecting the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    /// Protocol version the sender speaks. Compared against [`PROTOCOL_VERSION`].
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,

    /// The extension this message is to/from (the manifest `name`).
    #[serde(rename = "extensionId")]
    pub extension_id: ExtensionName,

    /// The direction-specific body.
    pub message: T,
}

impl<T> Envelope<T> {
    /// Wrap a body for `extension_id` at the current [`PROTOCOL_VERSION`].
    pub fn new(extension_id: ExtensionName, message: T) -> Envelope<T> {
        Envelope {
            protocol_version: PROTOCOL_VERSION,
            extension_id,
            message,
        }
    }

    /// Whether this envelope's version is compatible with the running protocol.
    ///
    /// Compatibility is exact-major: this crate is at major 1, so any other
    /// value is incompatible. (When a major 2 lands, this becomes a range
    /// check.)
    #[must_use]
    pub fn is_version_compatible(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
    }
}

/// A single transcript segment delivered to an extension.
///
/// Plain text and timing only. No audio, no embedding, no confidence vector —
/// just the committed words and where they sit in the meeting timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct TranscriptSegment {
    /// Stable segment id, for example `seg-042`.
    #[serde(rename = "segmentId")]
    pub segment_id: String,

    /// The committed transcript text for this segment.
    pub text: String,

    /// Resolved display speaker (from the rename map), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,

    /// Raw diarization label (`S1`), if available.
    #[serde(
        default,
        rename = "speakerRaw",
        skip_serializing_if = "Option::is_none"
    )]
    pub speaker_raw: Option<String>,

    /// Start offset, milliseconds from session start.
    #[serde(rename = "startMs")]
    pub start_ms: u64,

    /// End offset, milliseconds from session start.
    #[serde(rename = "endMs")]
    pub end_ms: u64,
}

/// The category of an extracted note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NoteCategory {
    /// A decision reached in the meeting.
    Decision,
    /// An action item.
    Action,
    /// A key point.
    Keypoint,
    /// An open question.
    Question,
}

/// A single extracted note delivered to an extension. Text only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct NoteItem {
    /// Stable note id, for example `note-017`.
    #[serde(rename = "noteId")]
    pub note_id: String,

    /// Which section this note belongs to.
    pub category: NoteCategory,

    /// The note text.
    pub text: String,

    /// Attributed speaker, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,

    /// When the note was surfaced, milliseconds from session start.
    #[serde(rename = "timestampMs")]
    pub timestamp_ms: u64,
}

/// Host -> extension messages (push + export reply).
///
/// `#[non_exhaustive]` so adding a message type is a minor-version change.
/// Tagged by `type` to match the `docs/EXTENSIONS.md` §4 envelope on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostMessage {
    /// A committed transcription segment (`transcript.update`).
    ///
    /// Requires `transcript.text` or `transcript.segments`.
    #[serde(rename = "transcript.update")]
    TranscriptUpdate(TranscriptSegment),

    /// A newly extracted note of any category (`notes.update`).
    ///
    /// Requires the corresponding `notes.*` capability.
    #[serde(rename = "notes.update")]
    NotesUpdate(NoteItem),

    /// A user renamed a speaker (`speaker.rename`). Requires `speaker.labels`.
    #[serde(rename = "speaker.rename")]
    SpeakerRename {
        /// Raw diarization label, for example `S1`.
        raw: String,
        /// New display name, for example `Alice`.
        display: String,
    },

    /// The meeting started (`meeting.start`). Requires `meeting.metadata`.
    #[serde(rename = "meeting.start")]
    MeetingStart {
        /// Stable meeting id.
        #[serde(rename = "meetingId")]
        meeting_id: String,
        /// Meeting title.
        title: String,
        /// Wall-clock start, epoch milliseconds.
        #[serde(rename = "startMs")]
        start_ms: u64,
    },

    /// The meeting stopped (`meeting.stop`). Requires `meeting.metadata`.
    #[serde(rename = "meeting.stop")]
    MeetingStop {
        /// Stable meeting id.
        #[serde(rename = "meetingId")]
        meeting_id: String,
        /// Total duration, milliseconds.
        #[serde(rename = "durationMs")]
        duration_ms: u64,
    },

    /// The reply to an [`ExtensionMessage::ExportRequest`] (`export.response`).
    ///
    /// Carries only the intersection of the request and the extension's granted
    /// data capabilities; see [`crate::permissions`].
    #[serde(rename = "export.response")]
    ExportResponse(ExportSnapshot),
}

/// A pull-style snapshot of the meeting data an extension is entitled to.
///
/// Each field is `None` unless the extension both requested it and is granted
/// the matching [`DataCapability`]. There is, by construction, no audio /
/// embedding / tensor field to populate.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct ExportSnapshot {
    /// Transcript segments, present only if a transcript capability is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Vec<TranscriptSegment>>,

    /// Notes, present only if at least one `notes.*` capability is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<NoteItem>>,

    /// Speaker rename map, present only if `speaker.labels` is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers: Option<Vec<SpeakerLabel>>,
}

/// One entry of the `S1 -> "Alice"` rename map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct SpeakerLabel {
    /// Raw diarization label, for example `S1`.
    pub raw: String,
    /// Display name the user assigned.
    pub display: String,
}

/// Extension -> host messages (requests the host validates against grants).
///
/// `#[non_exhaustive]`; tagged by `type` to match `docs/EXTENSIONS.md` §5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExtensionMessage {
    /// Ask the host to mount panel content (`render.panel`). The host sanitizes
    /// the HTML and renders it inside the panel iframe, never the main page.
    /// Requires the `panel` UI capability.
    #[serde(rename = "render.panel")]
    RenderPanel {
        /// HTML to render inside the sandboxed panel iframe.
        html: String,
    },

    /// Post a short, text-only toast (`render.notification`). Requires the
    /// `notification` UI capability.
    #[serde(rename = "render.notification")]
    RenderNotification {
        /// Toast text. Plain text; no HTML.
        text: String,
    },

    /// Request a snapshot of entitled meeting data (`export.request`). The host
    /// replies with [`HostMessage::ExportResponse`] carrying the intersection of
    /// `include` and the extension's granted capabilities.
    #[serde(rename = "export.request")]
    ExportRequest {
        /// Data surfaces the extension would like. Anything outside its grants
        /// is omitted from the response without error.
        include: Vec<DataCapability>,
    },
}
