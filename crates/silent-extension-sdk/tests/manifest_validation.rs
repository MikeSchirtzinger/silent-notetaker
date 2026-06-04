//! Deterministic manifest-validation tests, including adversarial manifests
//! (PRD R7 acceptance).
//!
//! The acceptance criterion "an extension requesting raw audio is rejected at
//! manifest validation" is proven here at the strongest possible layer: the
//! request does not survive *decode*, because raw audio is not a token in the
//! capability vocabulary. The remaining tests exercise the policy layer
//! (forbidden capability spelled various ways, wildcard origins, oversize,
//! duplicate names/tokens, path traversal).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the PRD lint \
              config allows this in tests"
)]

use silent_extension_sdk::capability::{DataCapability, NetworkGrant, UiCapability};
use silent_extension_sdk::manifest::{ExtensionName, Version};
use silent_extension_sdk::validation::{
    MAX_MANIFEST_BYTES, MAX_NETWORK_GRANTS, ManifestError, parse_and_validate,
};

/// A minimal valid manifest used as the baseline the adversarial cases mutate.
const VALID: &str = r#"{
  "name": "notion-export",
  "displayName": "Notion Export",
  "version": "0.1.0",
  "description": "Push decisions and action items to Notion.",
  "entrypoint": "index.js",
  "capabilities": {
    "data": ["notes.decisions", "notes.actions", "meeting.metadata"],
    "ui": ["panel", "notification"],
    "network": ["https://api.notion.com"]
  }
}"#;

#[test]
fn valid_manifest_parses_and_validates() {
    let m = parse_and_validate(VALID.as_bytes()).expect("baseline manifest is valid");
    assert_eq!(m.name, ExtensionName::parse("notion-export").unwrap());
    assert_eq!(m.version, Version::new(0, 1, 0));
    assert_eq!(m.display_name.as_deref(), Some("Notion Export"));
    assert!(
        m.capabilities
            .data
            .contains(&DataCapability::NotesDecisions)
    );
    assert!(m.capabilities.ui.contains(&UiCapability::Panel));
    assert_eq!(
        m.capabilities.network,
        vec![NetworkGrant::parse("https://api.notion.com").unwrap()]
    );
}

// ---------------------------------------------------------------------------
// The headline acceptance: forbidden capabilities cannot even be requested.
// ---------------------------------------------------------------------------

/// `transcript.raw_audio` is not a vocabulary token: decode fails. Proves "an
/// extension requesting raw audio is rejected at manifest validation".
#[test]
fn raw_audio_request_is_rejected_at_decode() {
    let raw = r#"{
      "name": "evil",
      "version": "1.0.0",
      "entrypoint": "index.js",
      "capabilities": { "data": ["transcript.raw_audio"] }
    }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("raw audio must be rejected");
    assert!(
        matches!(err, ManifestError::Decode { .. }),
        "expected a decode rejection, got {err:?}"
    );
}

/// Each forbidden data token (audio, embeddings, mel, activations, in several
/// plausible spellings) must fail to decode — there is no variant for any of
/// them.
#[test]
fn every_forbidden_data_token_fails_to_decode() {
    for token in [
        "audio",
        "audio.raw",
        "raw_audio",
        "transcript.raw_audio",
        "embeddings",
        "embedding",
        "voice.embedding",
        "mel",
        "mel.tensor",
        "mel_spectrogram",
        "activations",
        "model.activations",
        "tensor",
        "pcm",
    ] {
        let raw = format!(
            r#"{{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": {{ "data": ["{token}"] }} }}"#
        );
        match parse_and_validate(raw.as_bytes()) {
            Err(ManifestError::Decode { .. }) => {}
            Err(other) => panic!("token `{token}` should fail at decode, got {other:?}"),
            Ok(m) => panic!("forbidden token `{token}` must never decode; got {m:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Adversarial: wildcard origins.
// ---------------------------------------------------------------------------

#[test]
fn wildcard_origin_is_rejected_at_decode() {
    for origin in [
        "https://*.notion.com",
        "https://*",
        "*",
        "https://*.com",
        "*://api.notion.com",
    ] {
        let raw = format!(
            r#"{{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": {{ "network": ["{origin}"] }} }}"#
        );
        let err = parse_and_validate(raw.as_bytes())
            .expect_err(&format!("wildcard origin `{origin}` must be rejected"));
        assert!(
            matches!(err, ManifestError::Decode { .. }),
            "origin `{origin}` should fail at decode, got {err:?}"
        );
    }
}

#[test]
fn origin_with_path_or_userinfo_is_rejected_at_decode() {
    for origin in [
        "https://api.notion.com/v1",
        "https://api.notion.com/",
        "https://user@api.notion.com",
        "http://evil.com",
        "ftp://api.notion.com",
        "api.notion.com",
        "https://api.notion.com:notaport",
    ] {
        let raw = format!(
            r#"{{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": {{ "network": ["{origin}"] }} }}"#
        );
        let err = parse_and_validate(raw.as_bytes())
            .expect_err(&format!("malformed origin `{origin}` must be rejected"));
        assert!(matches!(err, ManifestError::Decode { .. }));
    }
}

#[test]
fn loopback_http_origin_is_allowed() {
    // The local Claude bridge runs on http://localhost — allow it explicitly.
    for origin in [
        "http://localhost:8765",
        "http://127.0.0.1:3000",
        "http://localhost",
    ] {
        let raw = format!(
            r#"{{ "name": "bridge", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": {{ "network": ["{origin}"] }} }}"#
        );
        parse_and_validate(raw.as_bytes())
            .unwrap_or_else(|e| panic!("loopback origin `{origin}` should be allowed, got {e:?}"));
    }
}

// ---------------------------------------------------------------------------
// Adversarial: oversize.
// ---------------------------------------------------------------------------

#[test]
fn oversize_manifest_is_rejected_before_parsing() {
    // A megabyte of padding inside a JSON string field. The size guard fires
    // before serde_json is handed the input.
    let padding = "A".repeat(MAX_MANIFEST_BYTES + 1);
    let raw = format!(
        r#"{{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
              "description": "{padding}" }}"#
    );
    let err = parse_and_validate(raw.as_bytes()).expect_err("oversize manifest must be rejected");
    match err {
        ManifestError::TooLarge { len, max } => {
            assert!(len > max);
            assert_eq!(max, MAX_MANIFEST_BYTES);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn too_many_network_grants_is_rejected() {
    let grants: Vec<String> = (0..=MAX_NETWORK_GRANTS)
        .map(|i| format!("\"https://h{i}.example.com\""))
        .collect();
    let raw = format!(
        r#"{{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
              "capabilities": {{ "network": [{}] }} }}"#,
        grants.join(", ")
    );
    let err = parse_and_validate(raw.as_bytes()).expect_err("too many grants must be rejected");
    match err {
        ManifestError::TooManyNetworkGrants { count, max } => {
            assert_eq!(count, MAX_NETWORK_GRANTS + 1);
            assert_eq!(max, MAX_NETWORK_GRANTS);
        }
        other => panic!("expected TooManyNetworkGrants, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Adversarial: duplicate names / tokens.
// ---------------------------------------------------------------------------

#[test]
fn duplicate_data_capability_is_rejected() {
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": { "data": ["notes.actions", "notes.actions"] } }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("duplicate data cap must be rejected");
    match err {
        ManifestError::DuplicateDataCapability { capability } => {
            assert_eq!(capability, "notes.actions");
        }
        other => panic!("expected DuplicateDataCapability, got {other:?}"),
    }
}

#[test]
fn duplicate_ui_capability_is_rejected() {
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": { "ui": ["panel", "panel"] } }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("duplicate ui cap must be rejected");
    assert!(matches!(
        err,
        ManifestError::DuplicateUiCapability { capability } if capability == "panel"
    ));
}

#[test]
fn duplicate_network_grant_is_rejected() {
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": { "network": ["https://a.com", "https://a.com"] } }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("duplicate grant must be rejected");
    assert!(matches!(
        err,
        ManifestError::DuplicateNetworkGrant { origin } if origin == "https://a.com"
    ));
}

// ---------------------------------------------------------------------------
// Adversarial: malformed names, versions, entrypoints, unknown fields.
// ---------------------------------------------------------------------------

#[test]
fn bad_name_is_rejected_at_decode() {
    for name in [
        "",
        "Has Space",
        "1startsdigit",
        "UPPER",
        "emoji😀",
        "has/slash",
    ] {
        let raw = format!(r#"{{ "name": "{name}", "version": "1.0.0", "entrypoint": "i.js" }}"#);
        let err =
            parse_and_validate(raw.as_bytes()).expect_err(&format!("name `{name}` must reject"));
        assert!(
            matches!(err, ManifestError::Decode { .. }),
            "name `{name}` should fail at decode, got {err:?}"
        );
    }
}

#[test]
fn bad_version_is_rejected_at_decode() {
    for version in [
        "1.0",
        "1.0.0.0",
        "1.0.x",
        "v1.0.0",
        "01.0.0",
        "1.0.0-rc.1",
        "",
    ] {
        let raw = format!(r#"{{ "name": "x", "version": "{version}", "entrypoint": "i.js" }}"#);
        let err = parse_and_validate(raw.as_bytes())
            .expect_err(&format!("version `{version}` must reject"));
        assert!(matches!(err, ManifestError::Decode { .. }));
    }
}

#[test]
fn empty_entrypoint_is_rejected() {
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "   " }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("empty entrypoint must reject");
    assert!(matches!(err, ManifestError::EmptyEntrypoint));
}

#[test]
fn absolute_or_url_entrypoint_is_rejected() {
    for entry in ["/abs.js", "https://evil.com/x.js"] {
        let raw = format!(r#"{{ "name": "x", "version": "1.0.0", "entrypoint": "{entry}" }}"#);
        let err = parse_and_validate(raw.as_bytes())
            .expect_err(&format!("entrypoint `{entry}` must reject"));
        assert!(matches!(err, ManifestError::AbsoluteEntrypoint { .. }));
    }
}

#[test]
fn traversal_entrypoint_is_rejected() {
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "../../etc/passwd" }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("traversal entrypoint must reject");
    assert!(matches!(err, ManifestError::EntrypointTraversal { .. }));
}

#[test]
fn unknown_top_level_field_is_rejected_at_decode() {
    // deny_unknown_fields: a manifest cannot smuggle meaning through fields the
    // host does not understand.
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "secretBackdoor": true }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("unknown field must reject");
    assert!(matches!(err, ManifestError::Decode { .. }));
}

#[test]
fn unknown_capability_axis_is_rejected_at_decode() {
    let raw = r#"{ "name": "x", "version": "1.0.0", "entrypoint": "i.js",
                  "capabilities": { "filesystem": ["/etc"] } }"#;
    let err = parse_and_validate(raw.as_bytes()).expect_err("unknown axis must reject");
    assert!(matches!(err, ManifestError::Decode { .. }));
}
