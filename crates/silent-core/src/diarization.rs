//! Diarization boundary contracts (PRD Phase 2, Appendix A rows 12–15).
//!
//! The typed messages the UI (Task F2) exchanges with the Rust diarization core
//! (`silent-diarization`): speaker rename, merge, merge-by-rename, stop-time
//! global recluster, and the label/relabel events the UI renders. Defined here
//! so F2 wires the UI against a CONTRACT, not invented shapes.
//!
//! Same conventions as [`crate::commands`] / [`crate::events`]: `#[non_exhaustive]`
//! tagged enums (`{ "tag": "...", "payload": ... }`), `snake_case`, ts-rs export
//! under `#[cfg(test)]`. Additive variants do not break the UI (it has a wildcard
//! `switch` arm per the A3 escape hatch).
//!
//! # Privacy (PRD R5/R7)
//!
//! These types carry speaker *labels* and *cluster ids* only — never raw audio
//! or embeddings. The 192-d embeddings live solely inside `silent-diarization`'s
//! utterance log and never cross this boundary.

use serde::{Deserialize, Serialize};

/// A command the UI issues to the diarization core (UI → core).
///
/// Mirrors the index.html interactions: click-to-rename a speaker tag
/// (`renameSpeaker`), merge a mis-split speaker (`mergeSpeaker` /
/// `maybeMergeOnRename`), and the stop-time recluster the app runs on Stop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiarizationCommand {
    /// Rename a speaker cluster (click-to-edit a tag or the speakers-bar input).
    /// The core may detect this is really a merge-by-rename and respond with a
    /// [`DiarizationEvent::MergeProposed`] instead of applying the rename
    /// (Appendix A row 14).
    RenameSpeaker {
        /// The speaker cluster id (for example `S2`).
        speaker_id: String,
        /// The new display name (empty clears the name back to the raw id).
        name: String,
    },

    /// Confirm a merge the UI offered (the `confirm()` dialog said yes). Folds
    /// `from_id` into `to_id` and relabels every transcript line.
    ConfirmMerge {
        /// The speaker being folded away.
        from_id: String,
        /// The surviving speaker it folds into.
        to_id: String,
    },

    /// Run the stop-time global recluster (Appendix A row 15, docs/DIARIZATION.md).
    /// Issued by the recording-session machine before the summary opens. The
    /// optional `threshold` overrides the configured recluster threshold (the
    /// console-tunable knob from DIARIZATION.md §3).
    GlobalRecluster {
        /// Cosine merge threshold override, or `None` for the configured default.
        threshold: Option<f32>,
    },
}

/// An event the diarization core emits to the UI (core → UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(tag = "tag", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiarizationEvent {
    /// A new utterance was assigned a speaker (the live `identify` result). The
    /// UI stamps the transcript line with this id/color (Appendix A row 12–13).
    SpeakerAssigned {
        /// The assigned speaker.
        speaker: SpeakerDescriptor,
        /// `true` if this assignment created a brand-new speaker cluster.
        is_new: bool,
    },

    /// A speaker's display name changed (a plain rename was applied). The UI
    /// updates every `.speaker-tag[data-speaker=…]` and the speakers bar.
    SpeakerRenamed {
        /// The speaker cluster id.
        speaker_id: String,
        /// The new display name (empty means show the raw id).
        name: String,
    },

    /// The core determined a committed rename is really a merge-by-rename
    /// (`maybeMergeOnRename`). The UI shows the confirm dialog; on yes it sends
    /// [`DiarizationCommand::ConfirmMerge`], on no it falls back to a plain
    /// rename (Appendix A row 14).
    MergeProposed {
        /// The speaker the user was renaming (would be folded away).
        from_id: String,
        /// The existing speaker the typed value matched (the merge target).
        to_id: String,
    },

    /// A merge was applied (manual or merge-by-rename). The UI retags every
    /// transcript line for `from_id` to `to_id` and removes the absorbed chip.
    MergeApplied {
        /// The folded-away speaker.
        from_id: String,
        /// The surviving speaker.
        to_id: String,
    },

    /// The stop-time global recluster ran. `relabel` is the `old_id → new_id`
    /// map the UI applies across all transcript tags and the speakers bar; an
    /// empty map means no merges were needed. `speakers` is the post-recluster
    /// speaker set (renames preserved). Appendix A row 15.
    Reclustered {
        /// The applied relabeling (empty if nothing changed).
        relabel: Vec<RelabelEntry>,
        /// The surviving speakers after the recluster (renames preserved).
        speakers: Vec<SpeakerDescriptor>,
    },
}

/// A speaker cluster as the UI needs to render it. Carries the label and color
/// only — never the centroid/embedding (PRD R5/R7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct SpeakerDescriptor {
    /// The cluster id (`S1`, `S2`, …).
    pub id: String,
    /// The user-supplied display name, or empty if unnamed (show the id).
    pub name: String,
    /// Hex color from the 8-color rotation (for example `#00d4aa`).
    pub color: String,
    /// Number of utterances assigned to this speaker.
    pub count: u32,
}

/// One `old_id → new_id` entry in a recluster relabel map. A `Vec` of these
/// (rather than a `HashMap`) keeps the TypeScript a plain array the UI can
/// iterate deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
pub struct RelabelEntry {
    /// The pre-recluster speaker id.
    pub old_id: String,
    /// The post-recluster canonical speaker id.
    pub new_id: String,
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap as the assertion mechanism (PRD lint config)"
)]
mod tests {
    use super::*;

    #[test]
    fn diarization_command_round_trips() {
        let cmd = DiarizationCommand::RenameSpeaker {
            speaker_id: "S2".into(),
            name: "Alice".into(),
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        assert!(
            json.contains("\"tag\":\"rename_speaker\""),
            "tagged: {json}"
        );
        let back: DiarizationCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cmd, back);
    }

    #[test]
    fn recluster_event_round_trips() {
        let ev = DiarizationEvent::Reclustered {
            relabel: vec![RelabelEntry {
                old_id: "S5".into(),
                new_id: "S1".into(),
            }],
            speakers: vec![SpeakerDescriptor {
                id: "S1".into(),
                name: "Alice".into(),
                color: "#00d4aa".into(),
                count: 7,
            }],
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"tag\":\"reclustered\""));
        let back: DiarizationEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }
}
