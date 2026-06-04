//! The permission-grant model (PRD R7, `docs/EXTENSIONS.md` §2).
//!
//! A [`Manifest`] *requests* capabilities; a [`GrantSet`] records what the user
//! *approved*. The two are distinct on purpose: installing an extension does not
//! auto-grant everything it asks for, and a user may later revoke individual
//! grants without editing the manifest.
//!
//! The grant set is the authority every boundary check consults:
//!
//! - **Data** — before the host dispatches a [`crate::protocol::HostMessage`] or
//!   fills an [`crate::protocol::ExportSnapshot`], it asks the grant set whether
//!   the relevant [`DataCapability`] is granted. Ungranted data is omitted, not
//!   errored (`docs/EXTENSIONS.md` §2 "the extension never knows what it did not
//!   declare").
//! - **UI** — a render request is honoured only if the matching [`UiCapability`]
//!   is granted; otherwise it is a silent no-op.
//! - **Network** — the granted origins are exactly the `connect-src` additions
//!   for that extension's worker/iframe, and nowhere else
//!   ([`GrantSet::connect_src`]).
//!
//! This module is the deterministic policy core. *Persistence* (IndexedDB) and
//! *CSP application* are the j2/j3 wiring layer; [`GrantSet`] is `serde`-round-
//! trippable so that layer can store and reload it verbatim, and exposes the
//! exact origin list that layer feeds to the CSP. No I/O happens here.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::capability::{DataCapability, NetworkGrant, UiCapability};
use crate::manifest::{ExtensionName, Manifest};

/// The set of capabilities a user has granted one extension.
///
/// Stored per extension (keyed by [`ExtensionName`]). The sets are sorted
/// (`BTreeSet`) so the serialized form is canonical and byte-stable for
/// persistence and diffing. A [`GrantSet`] built with [`GrantSet::new`] grants
/// nothing — the deny-by-default posture is the empty-set state. There is
/// deliberately no `Default` impl: a grant set is always bound to a specific
/// extension, never a nameless one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(deny_unknown_fields)]
pub struct GrantSet {
    /// The extension these grants apply to.
    pub extension: ExtensionName,

    /// Granted data surfaces.
    #[serde(default)]
    pub data: BTreeSet<DataCapability>,

    /// Granted UI surfaces.
    #[serde(default)]
    pub ui: BTreeSet<UiCapability>,

    /// Granted network origins. These — and only these — are the per-extension
    /// `connect-src` additions.
    #[serde(default)]
    pub network: BTreeSet<NetworkGrant>,
}

impl GrantSet {
    /// An empty grant set for `extension`: nothing granted (deny by default).
    #[must_use]
    pub fn new(extension: ExtensionName) -> GrantSet {
        GrantSet {
            extension,
            data: BTreeSet::new(),
            ui: BTreeSet::new(),
            network: BTreeSet::new(),
        }
    }

    /// Grant everything a manifest requested ("Allow" on the install screen).
    ///
    /// This is the all-or-nothing install path (`docs/EXTENSIONS.md` §2). The
    /// manifest is assumed already validated, so every capability it names is in
    /// the vocabulary. Returns a grant set that mirrors the manifest's requests.
    #[must_use]
    pub fn grant_all(manifest: &Manifest) -> GrantSet {
        GrantSet {
            extension: manifest.name.clone(),
            data: manifest.capabilities.data.iter().copied().collect(),
            ui: manifest.capabilities.ui.iter().copied().collect(),
            network: manifest.capabilities.network.iter().cloned().collect(),
        }
    }

    /// Whether a data capability is granted.
    #[must_use]
    pub fn has_data(&self, cap: DataCapability) -> bool {
        self.data.contains(&cap)
    }

    /// Whether a UI capability is granted.
    #[must_use]
    pub fn has_ui(&self, cap: UiCapability) -> bool {
        self.ui.contains(&cap)
    }

    /// Whether a network origin is granted.
    #[must_use]
    pub fn has_network(&self, origin: &NetworkGrant) -> bool {
        self.network.contains(origin)
    }

    /// Grant one data capability. Returns `true` if it was newly added.
    pub fn grant_data(&mut self, cap: DataCapability) -> bool {
        self.data.insert(cap)
    }

    /// Grant one UI capability. Returns `true` if it was newly added.
    pub fn grant_ui(&mut self, cap: UiCapability) -> bool {
        self.ui.insert(cap)
    }

    /// Grant one network origin. Returns `true` if it was newly added.
    pub fn grant_network(&mut self, origin: NetworkGrant) -> bool {
        self.network.insert(origin)
    }

    /// Revoke a data capability. Returns `true` if it was present.
    ///
    /// Revocation takes effect for the next boundary check; the host re-reads
    /// the grant set before each dispatch (`docs/EXTENSIONS.md` §2 "Revocation").
    pub fn revoke_data(&mut self, cap: DataCapability) -> bool {
        self.data.remove(&cap)
    }

    /// Revoke a UI capability. Returns `true` if it was present.
    pub fn revoke_ui(&mut self, cap: UiCapability) -> bool {
        self.ui.remove(&cap)
    }

    /// Revoke a network origin. Returns `true` if it was present. A revoked
    /// origin disappears from [`GrantSet::connect_src`] immediately, so the next
    /// CSP rebuild drops it.
    pub fn revoke_network(&mut self, origin: &NetworkGrant) -> bool {
        self.network.remove(origin)
    }

    /// Revoke everything. Used when a user removes the extension.
    pub fn revoke_all(&mut self) {
        self.data.clear();
        self.ui.clear();
        self.network.clear();
    }

    /// The exact `connect-src` origin list for this extension's worker/iframe.
    ///
    /// This is the *only* relaxation of the base page CSP for this extension,
    /// and it is scoped to this extension's context alone. An empty result means
    /// the extension may reach no network host at all (the default). The wiring
    /// layer (j2/j3) feeds this verbatim into the per-extension CSP; this crate
    /// computes it but never applies it.
    #[must_use]
    pub fn connect_src(&self) -> Vec<&str> {
        self.network.iter().map(NetworkGrant::as_str).collect()
    }

    /// Restrict every granted capability to what `manifest` still requests.
    ///
    /// If an extension updates and *drops* a capability from its manifest, the
    /// previously granted-but-no-longer-requested capability must not survive.
    /// Returns `true` if anything was dropped. Grants are never *added* here —
    /// that would re-require user consent.
    pub fn intersect_with_manifest(&mut self, manifest: &Manifest) -> bool {
        let req_data: BTreeSet<DataCapability> =
            manifest.capabilities.data.iter().copied().collect();
        let req_ui: BTreeSet<UiCapability> = manifest.capabilities.ui.iter().copied().collect();
        let req_net: BTreeSet<NetworkGrant> =
            manifest.capabilities.network.iter().cloned().collect();

        let before = self.data.len() + self.ui.len() + self.network.len();
        self.data.retain(|c| req_data.contains(c));
        self.ui.retain(|c| req_ui.contains(c));
        self.network.retain(|o| req_net.contains(o));
        let after = self.data.len() + self.ui.len() + self.network.len();
        before != after
    }
}

/// Filter a [`crate::protocol::ExportSnapshot`]'s requested surfaces to the
/// granted subset.
///
/// Returns the intersection of `requested` and the granted data capabilities,
/// in vocabulary order. This is the policy behind `export.response`: the host
/// fills only the granted surfaces and omits the rest without error
/// (`docs/EXTENSIONS.md` §5 `export.request`).
#[must_use]
pub fn granted_export_surfaces(
    grants: &GrantSet,
    requested: &[DataCapability],
) -> Vec<DataCapability> {
    DataCapability::ALL
        .iter()
        .copied()
        .filter(|c| requested.contains(c) && grants.has_data(*c))
        .collect()
}
