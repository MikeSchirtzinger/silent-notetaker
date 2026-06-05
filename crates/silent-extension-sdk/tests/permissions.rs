//! Deterministic permission-grant tests (PRD R7: granted set persisted,
//! revocable, applied per extension context).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the PRD lint \
              config allows this in tests"
)]

use silent_extension_sdk::capability::{DataCapability, NetworkGrant, UiCapability};
use silent_extension_sdk::manifest::ExtensionName;
use silent_extension_sdk::permissions::{GrantSet, granted_export_surfaces};
use silent_extension_sdk::validation::parse_and_validate;

const MANIFEST: &str = r#"{
  "name": "notion-export",
  "version": "0.1.0",
  "entrypoint": "index.js",
  "capabilities": {
    "data": ["notes.decisions", "notes.actions", "meeting.metadata"],
    "ui": ["panel", "notification"],
    "network": ["https://api.notion.com"]
  }
}"#;

#[test]
fn new_grant_set_grants_nothing() {
    let g = GrantSet::new(ExtensionName::parse("x").unwrap());
    assert!(g.data.is_empty());
    assert!(g.ui.is_empty());
    assert!(g.network.is_empty());
    // Deny-by-default: connect-src is empty, so the extension reaches no host.
    assert!(g.connect_src().is_empty());
    assert!(!g.has_data(DataCapability::TranscriptText));
}

#[test]
fn grant_all_mirrors_the_manifest() {
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let g = GrantSet::grant_all(&m);

    assert!(g.has_data(DataCapability::NotesDecisions));
    assert!(g.has_data(DataCapability::NotesActions));
    assert!(g.has_data(DataCapability::MeetingMetadata));
    assert!(!g.has_data(DataCapability::TranscriptText));

    assert!(g.has_ui(UiCapability::Panel));
    assert!(g.has_ui(UiCapability::Notification));

    let notion = NetworkGrant::parse("https://api.notion.com").unwrap();
    assert!(g.has_network(&notion));
    assert_eq!(g.connect_src(), vec!["https://api.notion.com"]);
}

#[test]
fn revocation_is_immediate_and_drops_from_connect_src() {
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let mut g = GrantSet::grant_all(&m);
    let notion = NetworkGrant::parse("https://api.notion.com").unwrap();

    assert!(g.revoke_network(&notion));
    assert!(!g.has_network(&notion));
    // The revoked origin is gone from the CSP derivation immediately.
    assert!(g.connect_src().is_empty());
    // Revoking again reports it was already absent.
    assert!(!g.revoke_network(&notion));

    assert!(g.revoke_ui(UiCapability::Panel));
    assert!(!g.has_ui(UiCapability::Panel));
    assert!(g.has_ui(UiCapability::Notification));

    assert!(g.revoke_data(DataCapability::NotesActions));
    assert!(!g.has_data(DataCapability::NotesActions));
}

#[test]
fn revoke_all_clears_everything() {
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let mut g = GrantSet::grant_all(&m);
    g.revoke_all();
    assert!(g.data.is_empty() && g.ui.is_empty() && g.network.is_empty());
}

#[test]
fn intersect_with_manifest_drops_no_longer_requested_grants() {
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let mut g = GrantSet::grant_all(&m);

    // The extension updates and now requests fewer capabilities.
    let shrunk = r#"{ "name": "notion-export", "version": "0.2.0", "entrypoint": "index.js",
                     "capabilities": { "data": ["notes.decisions"], "ui": ["panel"] } }"#;
    let m2 = parse_and_validate(shrunk.as_bytes()).unwrap();

    let dropped = g.intersect_with_manifest(&m2);
    assert!(dropped, "shrinking the manifest must drop grants");
    assert!(g.has_data(DataCapability::NotesDecisions));
    assert!(!g.has_data(DataCapability::NotesActions));
    assert!(!g.has_data(DataCapability::MeetingMetadata));
    assert!(g.has_ui(UiCapability::Panel));
    assert!(!g.has_ui(UiCapability::Notification));
    // The network grant was not re-requested, so it is gone.
    assert!(g.network.is_empty());

    // Intersecting again is a no-op (nothing left to drop).
    assert!(!g.intersect_with_manifest(&m2));
}

#[test]
fn intersect_never_adds_grants() {
    // A user who granted only a subset must not have the gap re-filled just
    // because the manifest still requests more.
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let mut g = GrantSet::new(m.name.clone());
    g.grant_data(DataCapability::NotesDecisions);

    let added = g.intersect_with_manifest(&m);
    assert!(
        !added,
        "intersection must not add the ungranted capabilities"
    );
    assert!(g.has_data(DataCapability::NotesDecisions));
    assert!(!g.has_data(DataCapability::NotesActions));
}

#[test]
fn export_surfaces_are_the_intersection_of_request_and_grant() {
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let g = GrantSet::grant_all(&m);

    // The extension requests more than it was granted; only the granted,
    // requested surfaces come back, in vocabulary order.
    let requested = [
        DataCapability::TranscriptText, // not granted -> omitted
        DataCapability::NotesActions,   // granted + requested -> kept
        DataCapability::NotesDecisions, // granted + requested -> kept
    ];
    let surfaces = granted_export_surfaces(&g, &requested);
    assert_eq!(
        surfaces,
        vec![DataCapability::NotesDecisions, DataCapability::NotesActions],
        "surfaces must be the granted intersection, in vocabulary order"
    );
}

#[test]
fn grant_set_round_trips_through_json_for_persistence() {
    // The wiring layer persists the grant set verbatim (IndexedDB). The
    // serialized form must round-trip byte-for-byte-equivalently.
    let m = parse_and_validate(MANIFEST.as_bytes()).unwrap();
    let g = GrantSet::grant_all(&m);

    let json = serde_json::to_string(&g).unwrap();
    let back: GrantSet = serde_json::from_str(&json).unwrap();
    assert_eq!(g, back);

    // And serializing twice is stable (BTreeSet -> canonical order).
    let json2 = serde_json::to_string(&back).unwrap();
    assert_eq!(json, json2);
}

#[test]
fn grant_set_rejects_unknown_persisted_fields() {
    // A tampered or future persisted record with an unknown field is rejected,
    // not silently accepted.
    let tampered = r#"{ "extension": "x", "data": [], "ui": [], "network": [], "smuggled": true }"#;
    assert!(serde_json::from_str::<GrantSet>(tampered).is_err());
}
