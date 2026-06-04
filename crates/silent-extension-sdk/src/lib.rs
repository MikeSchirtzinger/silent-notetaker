//! Extension SDK core (PRD Phase 6, R7).
//!
//! The capability-based, network-denied-by-default extension system for Silent
//! Notetaker. This crate is the *policy* core: the manifest schema, the closed
//! capability vocabulary, manifest validation, the versioned host <-> extension
//! message protocol, and the revocable permission-grant model. It is
//! deterministic, browser-free, and testable without a GPU or a Worker.
//!
//! The **sandboxed host** (the Worker / sandboxed-iframe that actually runs an
//! extension) and **CSP enforcement** (applying granted origins to a
//! per-extension `connect-src`, moving CSP from report-only to enforced) are the
//! j2/j3 wiring layer and live elsewhere. This crate gives that layer the typed
//! contracts to enforce; it performs no I/O and starts no Worker.
//!
//! # The privacy floor is the type system
//!
//! Silent Notetaker's promise is "private by architecture, not by policy"
//! (`docs/EXTENSIONS.md`). An extension marketplace is in direct tension with
//! that promise, so the boundary is enforced by the *absence of types*, not by a
//! runtime check that could be forgotten:
//!
//! - Raw audio samples, raw voice embeddings, mel-spectrogram tensors, and
//!   intermediate model activations are **not** variants of any capability enum
//!   in [`capability`]. There is no token for them, no `serde` alias, and no
//!   constructor. A manifest that asks for them fails to *decode* — "an
//!   extension requesting raw audio is rejected at manifest validation" (R7) is
//!   satisfied because the request cannot even be represented.
//! - The message protocol in [`protocol`] carries transcript text, notes,
//!   speaker labels, and meeting metadata, and *nothing else*. There is no field
//!   anywhere that transmits audio, embeddings, or tensors.
//! - Network is denied by default. A [`capability::NetworkGrant`] is a single,
//!   wildcard-free, origin-scoped grant; [`permissions::GrantSet`] is the only
//!   thing that relaxes a per-extension CSP, and only for the origins the user
//!   approved.
//!
//! # Modules
//!
//! - [`capability`] — the closed capability vocabulary ([`capability::DataCapability`],
//!   [`capability::UiCapability`], [`capability::NetworkGrant`]) and origin parsing.
//! - [`manifest`] — the [`manifest::Manifest`] schema, the validated
//!   [`manifest::ExtensionName`] and [`manifest::Version`] value types.
//! - [`validation`] — [`validation::parse_and_validate`]: size bound + decode +
//!   policy checks, with the precise [`validation::ManifestError`].
//! - [`protocol`] — the versioned [`protocol::Envelope`], [`protocol::HostMessage`]
//!   (host -> extension), and [`protocol::ExtensionMessage`] (extension -> host).
//! - [`permissions`] — [`permissions::GrantSet`]: the persisted, revocable
//!   granted set and the `connect-src` derivation.
//!
//! # TypeScript boundary
//!
//! Every boundary type derives [`ts_rs::TS`] under `#[cfg(test)]` and is exported
//! to this crate's `bindings/` directory by the `export_bindings` test, mirroring
//! `silent-core`. The committed bindings are the contract the (unchanged) JS host
//! and extension authors consume. Run:
//!
//! ```text
//! cargo test -p silent-extension-sdk export_bindings
//! ```
#![forbid(unsafe_code)]

pub mod capability;
pub mod manifest;
pub mod permissions;
pub mod protocol;
pub mod validation;

// ---------------------------------------------------------------------------
// TypeScript boundary export. Mirrors silent-core: the `TS` derive is gated to
// `#[cfg(test)]`, and this test writes one `.ts` per boundary type into
// `bindings/`. The committed output is the contract. Run with:
//
//     cargo test -p silent-extension-sdk export_bindings
//     git diff --exit-code crates/silent-extension-sdk/bindings/
// ---------------------------------------------------------------------------
#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "tests use expect as the assertion mechanism; the PRD lint config \
              allows this in tests"
)]
mod ts_bindings {
    use crate::capability::{DataCapability, NetworkGrant, NetworkGrantError, UiCapability};
    use crate::manifest::{
        Capabilities, ExtensionName, Manifest, NameError, Version, VersionError,
    };
    use crate::permissions::GrantSet;
    use crate::protocol::{
        Envelope, ExportSnapshot, ExtensionMessage, HostMessage, NoteCategory, NoteItem,
        SpeakerLabel, TranscriptSegment,
    };
    use crate::validation::ManifestError;
    use std::path::Path;
    use ts_rs::TS;

    #[test]
    fn export_bindings() {
        // `export_all` on each top-level type also exports every type it
        // references transitively, writing one `.ts` per type into the crate's
        // `bindings/` dir. The committed output is the boundary contract.
        macro_rules! export {
            ($($t:ty),+ $(,)?) => {
                $( <$t as TS>::export_all().expect(concat!("export ", stringify!($t))); )+
            };
        }

        export!(
            DataCapability,
            UiCapability,
            NetworkGrant,
            NetworkGrantError,
            Manifest,
            Capabilities,
            ExtensionName,
            NameError,
            Version,
            VersionError,
            ManifestError,
            // `Envelope<T>` is generic; export a concrete instantiation in each
            // direction so the generic `.ts` is emitted.
            Envelope<HostMessage>,
            Envelope<ExtensionMessage>,
            HostMessage,
            ExtensionMessage,
            ExportSnapshot,
            TranscriptSegment,
            NoteItem,
            NoteCategory,
            SpeakerLabel,
            GrantSet,
        );

        let bindings = Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings");
        assert!(
            bindings.is_dir(),
            "bindings dir should exist after export: {}",
            bindings.display()
        );
        for expected in [
            "Manifest.ts",
            "DataCapability.ts",
            "NetworkGrant.ts",
            "ManifestError.ts",
            "HostMessage.ts",
            "ExtensionMessage.ts",
            "GrantSet.ts",
        ] {
            let p = bindings.join(expected);
            assert!(p.is_file(), "expected binding not written: {}", p.display());
        }
    }
}
