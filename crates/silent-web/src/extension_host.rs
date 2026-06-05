//! Wasm-bindgen extension-host surface (PRD Phase 6, Task J2; R7).
//!
//! The browser-facing wrapper around the j1 extension SDK
//! ([`silent_extension_sdk`]). It is the *policy host*: install-time manifest
//! validation, grant-set persistence, the per-extension data/UI/network boundary
//! checks, and the versioned `postMessage` envelope contract. The JS glue
//! (`extension-host.js`) drives this object and runs each extension in a
//! sandboxed iframe; the wasm core never touches the DOM, an iframe, or a Worker.
//!
//! # The law-vs-hands split (PRD R2/R7)
//!
//! The SDK owns the *vocabulary and the policy*: which capability tokens exist
//! (raw audio / embeddings are not representable, so a manifest asking for them
//! fails to *decode* — `docs/EXTENSIONS.md` §1.1), what a valid manifest is
//! ([`silent_extension_sdk::validation::parse_and_validate`]), and what a grant
//! set authorises ([`GrantSet::has_data`] / [`GrantSet::has_ui`] /
//! [`GrantSet::connect_src`]). This module is the glue that:
//!
//! 1. validates a manifest at install and surfaces the precise
//!    [`ManifestError`] string verbatim to the consent UI;
//! 2. persists the approved [`GrantSet`] (serde round-trip) to IndexedDB via
//!    [`silent_storage`] — the *same store as meeting data* (`docs/EXTENSIONS.md`
//!    §2) — and reloads / revokes it;
//! 3. gates every outbound [`HostMessage`] on the relevant capability so
//!    ungranted data is *silently omitted* (never errored — the SDK note: "the
//!    extension never knows what it did not declare");
//! 4. fills an [`ExportSnapshot`]'s requested surfaces to the granted subset;
//! 5. wraps every message in a versioned [`Envelope`] and *rejects* an inbound
//!    envelope whose protocol major version disagrees (R7 acceptance: a forged
//!    protocol-v2 envelope is refused).
//!
//! # Wire format
//!
//! Every method speaks the same serde-JSON-string convention as the rest of the
//! `silent-web` boundary ([`crate::session`] / [`crate::exports`]): JSON strings
//! in, JSON strings (or `null`) out, so no `serde-wasm-bindgen` dep is needed and
//! the glue `JSON.parse`s the result. The shapes are the committed ts-rs bindings
//! in `crates/silent-extension-sdk/bindings/`.
//!
//! # Persistence shape
//!
//! A [`GrantSet`] is stored as its serde JSON string keyed by the extension name
//! in the `extensionGrants` store (schema v4). [`silent_storage`] treats the JSON
//! as opaque — only this module knows it is a `GrantSet`, so storage never
//! depends on the SDK.
//!
//! # wasm32-only
//!
//! Gated out of the native workspace build (see `lib.rs`); the policy logic it
//! wraps is exhaustively unit-tested *natively* inside `silent-extension-sdk`, so
//! `cargo check --workspace` stays browser-dep-free while the contract stays
//! covered.

use wasm_bindgen::prelude::*;

use silent_extension_sdk::capability::DataCapability;
use silent_extension_sdk::manifest::ExtensionName;
use silent_extension_sdk::permissions::{GrantSet, granted_export_surfaces};
use silent_extension_sdk::protocol::{
    Envelope, ExportSnapshot, ExtensionMessage, HostMessage, PROTOCOL_VERSION,
};
use silent_extension_sdk::validation::parse_and_validate;

/// Stringify any `Display` error into a rejected-`Promise` `JsValue`. A loud
/// failure — never a silent drop.
fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Serialize a value to a `JsValue` JSON string the glue `JSON.parse`s. Mirrors
/// the rest of the `silent-web` boundary.
fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, JsValue> {
    serde_json::to_string(v)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| err(format!("serialize: {e}")))
}

/// The gating verdict for an outbound [`HostMessage`]: which capability (if any)
/// it requires, or an explicit deny for a message the host does not recognise.
///
/// `#[non_exhaustive]` on the protocol enums means a *future* message type could
/// reach this host. The privacy floor demands it be DENIED by default, never
/// passed through ungated — an unknown push must not leak data an old host cannot
/// reason about. So the wildcard maps to [`Gate::Deny`].
enum Gate {
    /// Dispatch only if this data capability is granted.
    Needs(DataCapability),
    /// Dispatch unconditionally — the payload carries nothing ungranted by
    /// construction (a pre-filtered `export.response`).
    Allow,
    /// An unrecognised (future) message type: omit it (deny by default).
    Deny,
}

/// The capability a [`HostMessage`] requires before the host may dispatch it
/// (`docs/EXTENSIONS.md` §4). `notes.update` is gated by the note's category, so
/// it is handled by the caller (not here); every other known message maps to one
/// capability, and any unknown future message is denied.
fn host_message_gate(msg: &HostMessage) -> Gate {
    match msg {
        // transcript.update needs transcript.text (the base requirement the docs
        // name first; transcript.segments is the richer superset).
        HostMessage::TranscriptUpdate(_) => Gate::Needs(DataCapability::TranscriptText),
        // speaker.rename needs speaker.labels.
        HostMessage::SpeakerRename { .. } => Gate::Needs(DataCapability::SpeakerLabels),
        // meeting lifecycle needs meeting.metadata.
        HostMessage::MeetingStart { .. } | HostMessage::MeetingStop { .. } => {
            Gate::Needs(DataCapability::MeetingMetadata)
        }
        // notes.update is gated per-category by the caller before this is reached.
        // export.response is already filtered to the granted subset by
        // `granted_export_surfaces`, so re-emitting it carries nothing ungranted
        // by construction → allow.
        HostMessage::NotesUpdate(_) | HostMessage::ExportResponse(_) => Gate::Allow,
        // A future, unknown message type: deny by default (privacy floor).
        _ => Gate::Deny,
    }
}

/// Map a note category to the `notes.*` data capability it requires
/// (`docs/EXTENSIONS.md` §4 `notes.update`). An unknown future category is
/// denied by returning `None`.
fn note_category_capability(
    category: silent_extension_sdk::protocol::NoteCategory,
) -> Option<DataCapability> {
    use silent_extension_sdk::protocol::NoteCategory as C;
    match category {
        C::Decision => Some(DataCapability::NotesDecisions),
        C::Action => Some(DataCapability::NotesActions),
        C::Keypoint => Some(DataCapability::NotesKeypoints),
        C::Question => Some(DataCapability::NotesQuestions),
        // A future note category with no mapped capability: deny by default.
        _ => None,
    }
}

/// Validate a manifest's bytes and decode the persisted grant set together.
/// Used by the gating methods so a revoked/absent grant set denies everything.
fn load_grants(grant_json: &str) -> Result<GrantSet, JsValue> {
    serde_json::from_str::<GrantSet>(grant_json).map_err(|e| err(format!("grant decode: {e}")))
}

// ---------------------------------------------------------------------------
// Install + consent
// ---------------------------------------------------------------------------

/// Validate a raw `manifest.json` for the install consent screen.
///
/// Runs [`parse_and_validate`] (size bound + serde decode + policy checks). On
/// success returns the *parsed* manifest JSON — the glue renders the consent
/// screen from it (display name, version, requested data/UI capabilities, and the
/// network grants verbatim). On failure REJECTS with the precise
/// [`ManifestError`] string, which the consent UI shows verbatim (R7 acceptance:
/// a manifest requesting an unknown capability — or a wildcard origin, oversize
/// doc, bad entrypoint — is rejected here with the exact reason).
///
/// This performs NO persistence: validating is not installing. The user must
/// approve, after which the glue calls [`commit_install`].
///
/// # Errors
///
/// Rejects with the [`ManifestError`] string if the manifest is invalid.
#[wasm_bindgen(js_name = validateManifest)]
pub fn validate_manifest(manifest_json: &str) -> Result<JsValue, JsValue> {
    let manifest = parse_and_validate(manifest_json.as_bytes()).map_err(err)?;
    to_js(&manifest)
}

/// Approve and persist an extension install (the consent screen's "Allow").
///
/// Re-validates the manifest (defence in depth — never trust a manifest that was
/// not re-checked at the moment of grant), builds the all-or-nothing
/// [`GrantSet::grant_all`] (every capability the manifest declared), and persists
/// it to IndexedDB. Returns the persisted grant set JSON so the glue can wire the
/// extension's iframe + per-extension `connect-src` immediately.
///
/// # Errors
///
/// Rejects if the manifest is invalid or the write fails.
#[wasm_bindgen(js_name = commitInstall)]
pub async fn commit_install(manifest_json: String) -> Result<JsValue, JsValue> {
    console_error_panic_hook::set_once();
    let manifest = parse_and_validate(manifest_json.as_bytes()).map_err(err)?;
    let grants = GrantSet::grant_all(&manifest);
    let grant_json =
        serde_json::to_string(&grants).map_err(|e| err(format!("grant serialize: {e}")))?;
    silent_storage::writer::save_extension_grant(manifest.name.as_str(), &grant_json)
        .await
        .map_err(err)?;
    Ok(JsValue::from_str(&grant_json))
}

/// Load one installed extension's persisted grant set (`null` if not installed).
///
/// # Errors
///
/// Rejects on a read or decode failure.
#[wasm_bindgen(js_name = loadGrantSet)]
pub async fn load_grant_set(name: String) -> Result<JsValue, JsValue> {
    // Validate the lookup key so a malformed name never reaches storage.
    ExtensionName::parse(name.clone()).map_err(err)?;
    match silent_storage::writer::load_extension_grant(&name)
        .await
        .map_err(err)?
    {
        Some(json) => Ok(JsValue::from_str(&json)),
        None => Ok(JsValue::NULL),
    }
}

/// Load every installed extension's grant set, as a JSON array. Used to
/// re-hydrate the host (mount each extension's iframe + CSP) on boot and to
/// render the extension-manager list.
///
/// # Errors
///
/// Rejects on a read failure.
#[wasm_bindgen(js_name = loadAllGrantSets)]
pub async fn load_all_grant_sets() -> Result<JsValue, JsValue> {
    let rows = silent_storage::writer::load_all_extension_grants()
        .await
        .map_err(err)?;
    // Decode each stored JSON string back into a GrantSet, drop any that fail to
    // decode (a corrupt row never breaks the manager), and re-emit as one JSON
    // array the glue parses.
    let sets: Vec<GrantSet> = rows
        .iter()
        .filter_map(|j| serde_json::from_str::<GrantSet>(j).ok())
        .collect();
    to_js(&sets)
}

/// Revoke an extension entirely (the manager's "Remove"): delete its grant set
/// so the next boundary check denies everything and the per-extension
/// `connect-src` drops to nothing.
///
/// # Errors
///
/// Rejects on a delete failure.
#[wasm_bindgen(js_name = revokeExtension)]
pub async fn revoke_extension(name: String) -> Result<(), JsValue> {
    ExtensionName::parse(name.clone()).map_err(err)?;
    silent_storage::writer::delete_extension_grant(&name)
        .await
        .map_err(err)
}

// ---------------------------------------------------------------------------
// Runtime boundary checks
// ---------------------------------------------------------------------------

/// The per-extension `connect-src` origin list for the CSP (j3 will apply it).
///
/// Decodes the persisted grant set and returns exactly [`GrantSet::connect_src`]
/// — the granted network origins and nothing else. An extension with no network
/// grant returns `[]` (network denied, the default).
///
/// # Errors
///
/// Rejects if the grant JSON cannot be decoded.
#[wasm_bindgen(js_name = connectSrc)]
pub fn connect_src(grant_json: &str) -> Result<JsValue, JsValue> {
    let grants = load_grants(grant_json)?;
    to_js(&grants.connect_src())
}

/// Gate one outbound [`HostMessage`] against a grant set, wrapping the result in
/// a versioned [`Envelope`] ready for `iframe.postMessage`.
///
/// Returns the envelope JSON if the message's required capability is granted, or
/// `null` if it is not — the data is *silently omitted*, never errored
/// (`docs/EXTENSIONS.md` §2). For a `notes.update` the gate is the note's
/// category-specific `notes.*` capability; for everything else it is the message
/// type's capability per [`required_data_capability`]. A message with no gated
/// payload always passes.
///
/// # Errors
///
/// Rejects only if the grant set or message JSON is malformed (a programming
/// error in the host) — never for an ungranted capability, which yields `null`.
#[wasm_bindgen(js_name = gateHostMessage)]
pub fn gate_host_message(grant_json: &str, message_json: &str) -> Result<JsValue, JsValue> {
    let grants = load_grants(grant_json)?;
    let msg: HostMessage =
        serde_json::from_str(message_json).map_err(|e| err(format!("message decode: {e}")))?;

    let allowed = match &msg {
        HostMessage::NotesUpdate(note) => match note_category_capability(note.category) {
            Some(cap) => grants.has_data(cap),
            None => false,
        },
        other => match host_message_gate(other) {
            Gate::Needs(cap) => grants.has_data(cap),
            Gate::Allow => true,
            Gate::Deny => false,
        },
    };
    if !allowed {
        return Ok(JsValue::NULL);
    }

    let envelope = Envelope::new(grants.extension.clone(), msg);
    to_js(&envelope)
}

/// Build the `export.response` snapshot for a pull-style `export.request`.
///
/// Given the extension's grant set and the `include` list from its request,
/// returns the *granted subset* of requested surfaces as JSON
/// (`{ transcript, notes, speakers }` flags via the granted [`DataCapability`]
/// list) — the host then fills only those surfaces. Anything outside the
/// intersection is omitted without error (`docs/EXTENSIONS.md` §5). This returns
/// the *granted-surface list*, not the data itself (the host owns the live note /
/// transcript text and fills it under these keys).
///
/// # Errors
///
/// Rejects if the grant set or include list is malformed.
#[wasm_bindgen(js_name = grantedExportSurfaces)]
pub fn granted_export_surfaces_js(
    grant_json: &str,
    include_json: &str,
) -> Result<JsValue, JsValue> {
    let grants = load_grants(grant_json)?;
    let requested: Vec<DataCapability> =
        serde_json::from_str(include_json).map_err(|e| err(format!("include decode: {e}")))?;
    let surfaces = granted_export_surfaces(&grants, &requested);
    to_js(&surfaces)
}

/// Wrap a host-built [`ExportSnapshot`] in a versioned `export.response`
/// [`Envelope`] for delivery to the extension iframe.
///
/// The host fills the snapshot (only the surfaces [`granted_export_surfaces_js`]
/// returned) and passes it here to get the versioned, extension-routed envelope.
///
/// # Errors
///
/// Rejects if the extension name or snapshot JSON is malformed.
#[wasm_bindgen(js_name = wrapExportResponse)]
pub fn wrap_export_response(name: &str, snapshot_json: &str) -> Result<JsValue, JsValue> {
    let extension = ExtensionName::parse(name.to_owned()).map_err(err)?;
    let snapshot: ExportSnapshot =
        serde_json::from_str(snapshot_json).map_err(|e| err(format!("snapshot decode: {e}")))?;
    let envelope = Envelope::new(extension, HostMessage::ExportResponse(snapshot));
    to_js(&envelope)
}

/// Validate an INBOUND envelope from an extension iframe and return its body.
///
/// Parses the [`Envelope<ExtensionMessage>`] and, crucially, REJECTS a protocol
/// major-version mismatch (R7 acceptance: a forged protocol-v2 envelope is
/// refused). On success returns `{ extensionId, message }` JSON the glue routes
/// (render.panel / render.notification / export.request). The host then re-checks
/// the relevant UI/data capability against the grant set before acting.
///
/// # Errors
///
/// Rejects with a precise reason if the envelope cannot be decoded OR speaks an
/// incompatible protocol version — the version refusal carries both the seen and
/// the expected version so the failure is loud and self-explaining.
#[wasm_bindgen(js_name = readExtensionEnvelope)]
pub fn read_extension_envelope(envelope_json: &str) -> Result<JsValue, JsValue> {
    let envelope: Envelope<ExtensionMessage> =
        serde_json::from_str(envelope_json).map_err(|e| err(format!("envelope decode: {e}")))?;

    if !envelope.is_version_compatible() {
        return Err(err(format!(
            "extension protocol version mismatch: envelope is v{}, host speaks v{} — message refused",
            envelope.protocol_version, PROTOCOL_VERSION
        )));
    }

    to_js(&InboundJson {
        extension_id: &envelope.extension_id,
        message: &envelope.message,
    })
}

/// The `{ extensionId, message }` shape [`read_extension_envelope`] returns to
/// the glue after a version-compatible inbound envelope is unwrapped.
#[derive(serde::Serialize)]
struct InboundJson<'a> {
    #[serde(rename = "extensionId")]
    extension_id: &'a ExtensionName,
    message: &'a ExtensionMessage,
}

/// Whether a UI capability is granted (the host calls this before honouring a
/// `render.panel` / `render.notification` request; an ungranted surface is a
/// silent no-op per `docs/EXTENSIONS.md` §2).
///
/// `ui` is the wire token (`"panel"` / `"notification"`).
///
/// # Errors
///
/// Rejects if the grant set or UI token is malformed.
#[wasm_bindgen(js_name = hasUiGrant)]
pub fn has_ui_grant(grant_json: &str, ui: &str) -> Result<bool, JsValue> {
    let grants = load_grants(grant_json)?;
    // Decode the UI token through the SDK's kebab-case serde so an unknown token
    // is a loud error, never a silent "ungranted".
    let cap: silent_extension_sdk::capability::UiCapability =
        serde_json::from_str(&format!("\"{ui}\"")).map_err(|e| err(format!("ui token: {e}")))?;
    Ok(grants.has_ui(cap))
}

/// The current host protocol version (so the glue can stamp its own messages and
/// surface the version it speaks in the manager UI).
#[wasm_bindgen(js_name = protocolVersion)]
#[must_use]
pub fn protocol_version() -> u32 {
    PROTOCOL_VERSION
}
