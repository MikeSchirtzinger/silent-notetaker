//! Deterministic host <-> extension protocol tests (PRD R7: versioned API
//! contracts; `docs/EXTENSIONS.md` §4/§5 wire format).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the PRD lint \
              config allows this in tests"
)]

use serde_json::json;
use silent_extension_sdk::capability::DataCapability;
use silent_extension_sdk::manifest::ExtensionName;
use silent_extension_sdk::protocol::{
    Envelope, ExtensionMessage, HostMessage, NoteCategory, NoteItem, PROTOCOL_VERSION,
    TranscriptSegment,
};

#[test]
fn envelope_carries_the_protocol_version() {
    let env = Envelope::new(
        ExtensionName::parse("notion-export").unwrap(),
        HostMessage::SpeakerRename {
            raw: "S1".into(),
            display: "Alice".into(),
        },
    );
    assert_eq!(env.protocol_version, PROTOCOL_VERSION);
    assert!(env.is_version_compatible());
}

#[test]
fn envelope_version_mismatch_is_detected() {
    let wire = json!({
        "protocolVersion": 999,
        "extensionId": "notion-export",
        "message": { "type": "speaker.rename", "payload": { "raw": "S1", "display": "Alice" } }
    });
    let env: Envelope<HostMessage> = serde_json::from_value(wire).unwrap();
    assert!(
        !env.is_version_compatible(),
        "version 999 must be incompatible"
    );
}

#[test]
fn transcript_update_matches_the_documented_wire_shape() {
    // From docs/EXTENSIONS.md §4 `transcript.update`.
    let msg = HostMessage::TranscriptUpdate(TranscriptSegment {
        segment_id: "seg-042".into(),
        text: "We should ship the CSP change before the launch.".into(),
        speaker: Some("Alice".into()),
        speaker_raw: Some("S1".into()),
        start_ms: 183_400,
        end_ms: 187_200,
    });
    let v = serde_json::to_value(&msg).unwrap();
    assert_eq!(v["type"], "transcript.update");
    assert_eq!(v["payload"]["segmentId"], "seg-042");
    assert_eq!(v["payload"]["speakerRaw"], "S1");
    assert_eq!(v["payload"]["startMs"], 183_400);

    // Round-trips.
    let back: HostMessage = serde_json::from_value(v).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn notes_update_matches_the_documented_wire_shape() {
    let msg = HostMessage::NotesUpdate(NoteItem {
        note_id: "note-017".into(),
        category: NoteCategory::Decision,
        text: "Ship the CSP change before the HN launch.".into(),
        speaker: Some("Alice".into()),
        timestamp_ms: 187_200,
    });
    let v = serde_json::to_value(&msg).unwrap();
    assert_eq!(v["type"], "notes.update");
    assert_eq!(v["payload"]["category"], "decision");
    assert_eq!(v["payload"]["noteId"], "note-017");

    let back: HostMessage = serde_json::from_value(v).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn export_request_uses_the_capability_vocabulary() {
    // The `include` list is typed against DataCapability, so it can only ever
    // name vocabulary tokens — never raw audio.
    let msg = ExtensionMessage::ExportRequest {
        include: vec![DataCapability::NotesDecisions, DataCapability::NotesActions],
    };
    let v = serde_json::to_value(&msg).unwrap();
    assert_eq!(v["type"], "export.request");
    assert_eq!(
        v["payload"]["include"],
        json!(["notes.decisions", "notes.actions"])
    );

    let back: ExtensionMessage = serde_json::from_value(v).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn export_request_with_forbidden_token_fails_to_decode() {
    // A crafted message asking for raw audio cannot decode: the token is not a
    // DataCapability variant. The protocol boundary mirrors the manifest floor.
    let wire = json!({
        "type": "export.request",
        "payload": { "include": ["transcript.raw_audio"] }
    });
    assert!(serde_json::from_value::<ExtensionMessage>(wire).is_err());
}

#[test]
fn render_panel_round_trips() {
    let msg = ExtensionMessage::RenderPanel {
        html: "<p>Notion status: connected</p>".into(),
    };
    let v = serde_json::to_value(&msg).unwrap();
    assert_eq!(v["type"], "render.panel");
    let back: ExtensionMessage = serde_json::from_value(v).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn unknown_message_type_fails_to_decode() {
    // A non_exhaustive enum still rejects an unknown tag on the wire.
    let wire = json!({ "type": "exfiltrate.audio", "payload": {} });
    assert!(serde_json::from_value::<HostMessage>(wire.clone()).is_err());
    assert!(serde_json::from_value::<ExtensionMessage>(wire).is_err());
}

#[test]
fn envelope_rejects_unknown_fields() {
    let wire = json!({
        "protocolVersion": 1,
        "extensionId": "x",
        "message": { "type": "render.panel", "payload": { "html": "<p>hi</p>" } },
        "smuggled": "audio-blob"
    });
    assert!(serde_json::from_value::<Envelope<ExtensionMessage>>(wire).is_err());
}
