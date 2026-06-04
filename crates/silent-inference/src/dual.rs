//! Dual-mode draft/refine coordination policy, as a Rust policy module.
//!
//! Ports `index.html`'s Dual-mode interleaving (Appendix A row 11: *"Dual mode:
//! Moonshine instant drafts + SenseVoice refined pass"*) into deterministic,
//! browser-free Rust policy (PRD R2, Phase 5). Dual runs **two** engines at once:
//! Moonshine (the [`crate::whisper_stream`] loop at 3 s chunks for fast feedback)
//! produces *drafts*, and SenseVoice (the [`crate::sensevoice`] segmentation
//! policy) produces *refined finals*. The UI shows Moonshine's drafts instantly
//! (dim/italic) and SenseVoice supersedes them with accurate text.
//!
//! # The coordination rule (what this module owns)
//!
//! From `index.html`:
//!
//! - **Moonshine final → draft** (`startDualModel`: the worker `final` is routed
//!   to `onPartial`; `handlePartial` dual branch renders it as a draft item):
//!   ```text
//!   renderTranscriptItem(timestamp, text.trim(), true); // true = draft
//!   ```
//! - **SenseVoice final → refine** (`handleFinal` dual branch): before rendering
//!   the refined (non-draft) item, remove the **older** drafts, keeping at most
//!   **one** as a preview of upcoming content:
//!   ```text
//!   const drafts = container.querySelectorAll('.transcript-draft');
//!   const toRemove = [...drafts].slice(0, Math.max(0, drafts.length - 1));
//!   toRemove.forEach(d => d.remove());
//!   // …then render the refined NON-draft item
//!   ```
//! - **Both guard empty text**: `handlePartial` early-returns on `!text.trim()`;
//!   `handleFinal` early-returns on `!text.trim() || !meetingId` (recording must
//!   be active). This coordinator assumes recording is active and applies the
//!   `text.trim()` guard.
//!
//! These are pure decisions over the *transcript-item list* — which items are
//! drafts, which to supersede, what order they sit in. The list itself is the
//! UI's, but the **policy that mutates it** is ported here and proven against a
//! DOM-free JS reference generator (`goldens/gen/dual_ref.mjs` →
//! `goldens/dual/interleavings.json`), event-for-event.
//!
//! # What stays elsewhere
//!
//! - Chunking / VAD / dedup for the Moonshine leg is [`crate::whisper_stream`]
//!   (`MOONSHINE_DUAL` config). VAD segmentation for the SenseVoice leg is
//!   [`crate::sensevoice`]. This module is purely the *interleaving* on top.
//! - Rendering (italic draft styling, DOM nodes) stays in JS. This module decides
//!   the list contents; the UI mirrors them.
//!
//! No `unwrap`/`expect`; no fallible op on the hot path (PRD "Rust engineering bar").

use serde::{Deserialize, Serialize};

/// One transcript item in the Dual-mode list: the refined/draft text and whether
/// it is a (superseded-on-refine) Moonshine draft.
///
/// Mirrors the DOM `.transcript-item` / `.transcript-draft` distinction as data.
/// The UI renders `draft == true` items dim/italic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptItem {
    /// The (trimmed) transcript text.
    pub text: String,
    /// `true` for a Moonshine draft (provisional, may be superseded); `false` for
    /// a SenseVoice refined final.
    pub draft: bool,
}

/// A typed mutation the coordinator emits so the UI can mirror its list edits
/// incrementally (rather than re-rendering the whole list). The UI applies these
/// to its DOM; the coordinator holds the authoritative list.
///
/// `#[non_exhaustive]` for additive ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ListEdit {
    /// Append a transcript item at the end of the list (the JS
    /// `renderTranscriptItem` append). Carries the appended item.
    Append {
        /// The item appended.
        item: TranscriptItem,
    },
    /// Remove the item at `index` (the JS draft `.remove()`). Removals for one
    /// refine are emitted in **ascending index order as of the pre-edit list**;
    /// the UI must apply them accounting for shifts, or apply the resulting
    /// [`DualCoordinator::items`] snapshot directly. (The golden test applies the
    /// snapshot, which is unambiguous.)
    Remove {
        /// Index (into the list *before this refine's removals*) of the draft to
        /// drop.
        index: usize,
    },
}

/// The Dual-mode draft/refine coordinator.
///
/// Feed it Moonshine drafts ([`on_moonshine_final`]) and SenseVoice refined
/// finals ([`on_sensevoice_final`]); it maintains the authoritative transcript
/// list and returns the [`ListEdit`]s for each event. The list it holds
/// ([`items`]) is the source of truth the UI mirrors.
///
/// [`on_moonshine_final`]: DualCoordinator::on_moonshine_final
/// [`on_sensevoice_final`]: DualCoordinator::on_sensevoice_final
/// [`items`]: DualCoordinator::items
#[derive(Debug, Clone, Default)]
pub struct DualCoordinator {
    items: Vec<TranscriptItem>,
}

impl DualCoordinator {
    /// A fresh coordinator with an empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The authoritative transcript list (drafts + refined finals, in order).
    #[must_use]
    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    /// Moonshine produced a final (its worker `final` message). In Dual mode this
    /// renders as a **draft** item appended to the list — unless the text is
    /// empty/whitespace, in which case it is ignored (JS `if (!text.trim()) return`).
    ///
    /// Returns the [`ListEdit`]s (a single `Append`, or none for empty text).
    pub fn on_moonshine_final(&mut self, text: &str) -> Vec<ListEdit> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let item = TranscriptItem {
            text: trimmed.to_owned(),
            draft: true,
        };
        self.items.push(item.clone());
        vec![ListEdit::Append { item }]
    }

    /// SenseVoice produced a refined final. In Dual mode this **supersedes** the
    /// older drafts: every draft currently in the list except the **last** is
    /// removed (keeping one as a preview), then the refined NON-draft item is
    /// appended. Empty/whitespace text is ignored (JS `if (!text.trim() || …) return`).
    ///
    /// Returns the [`ListEdit`]s: the draft removals (ascending pre-edit index),
    /// then the refined `Append`. None for empty text.
    pub fn on_sensevoice_final(&mut self, text: &str) -> Vec<ListEdit> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        // Indices of all draft items, in order (JS `querySelectorAll('.transcript-draft')`
        // in document order).
        let draft_indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| it.draft.then_some(i))
            .collect();

        // `toRemove = drafts.slice(0, Math.max(0, drafts.length - 1))`: all drafts
        // except the last → keep at most one as a preview.
        let remove_count = draft_indices.len().saturating_sub(1);
        let to_remove: Vec<usize> = draft_indices.into_iter().take(remove_count).collect();

        let mut edits: Vec<ListEdit> = to_remove
            .iter()
            .map(|&index| ListEdit::Remove { index })
            .collect();

        // Apply the removals to the authoritative list. `to_remove` is ascending,
        // so removing from a retained-index set in one pass (filter) avoids the
        // shifting hazard of sequential `Vec::remove`.
        let remove_set: std::collections::HashSet<usize> = to_remove.into_iter().collect();
        let mut idx = 0usize;
        self.items.retain(|_| {
            let keep = !remove_set.contains(&idx);
            idx += 1;
            keep
        });

        // Append the refined non-draft item.
        let item = TranscriptItem {
            text: trimmed.to_owned(),
            draft: false,
        };
        self.items.push(item.clone());
        edits.push(ListEdit::Append { item });
        edits
    }

    /// Reset to an empty list (new meeting / fresh session).
    pub fn reset(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap/expect as the assertion mechanism; the workspace \
              lint config permits this in test code (PRD 'Rust engineering bar')"
)]
mod tests {
    use super::*;

    fn draft(text: &str) -> TranscriptItem {
        TranscriptItem {
            text: text.into(),
            draft: true,
        }
    }
    fn refined(text: &str) -> TranscriptItem {
        TranscriptItem {
            text: text.into(),
            draft: false,
        }
    }

    #[test]
    fn moonshine_final_appends_a_draft() {
        let mut c = DualCoordinator::new();
        let edits = c.on_moonshine_final("hello world");
        assert_eq!(
            edits,
            vec![ListEdit::Append {
                item: draft("hello world")
            }]
        );
        assert_eq!(c.items(), &[draft("hello world")]);
    }

    #[test]
    fn moonshine_trims_and_ignores_empty() {
        let mut c = DualCoordinator::new();
        assert!(c.on_moonshine_final("").is_empty());
        assert!(c.on_moonshine_final("   ").is_empty());
        assert!(c.on_moonshine_final("\t\n ").is_empty());
        assert!(c.items().is_empty());
        let edits = c.on_moonshine_final("  trimmed  ");
        assert_eq!(
            edits,
            vec![ListEdit::Append {
                item: draft("trimmed")
            }]
        );
    }

    #[test]
    fn single_draft_survives_refine_as_preview() {
        // removeCount = max(0, 1-1) = 0 → the one draft stays, refined appended.
        let mut c = DualCoordinator::new();
        c.on_moonshine_final("draft");
        let edits = c.on_sensevoice_final("refined");
        assert_eq!(
            edits,
            vec![ListEdit::Append {
                item: refined("refined")
            }]
        );
        assert_eq!(c.items(), &[draft("draft"), refined("refined")]);
    }

    #[test]
    fn multiple_drafts_collapse_to_last_preview_on_refine() {
        let mut c = DualCoordinator::new();
        c.on_moonshine_final("one");
        c.on_moonshine_final("two");
        c.on_moonshine_final("three");
        // drafts at indices 0,1,2 → remove 0 and 1, keep index 2 ("three").
        let edits = c.on_sensevoice_final("refined");
        assert_eq!(
            edits,
            vec![
                ListEdit::Remove { index: 0 },
                ListEdit::Remove { index: 1 },
                ListEdit::Append {
                    item: refined("refined")
                },
            ]
        );
        assert_eq!(c.items(), &[draft("three"), refined("refined")]);
    }

    #[test]
    fn refine_with_no_drafts_just_appends() {
        let mut c = DualCoordinator::new();
        let edits = c.on_sensevoice_final("first refined");
        assert_eq!(
            edits,
            vec![ListEdit::Append {
                item: refined("first refined")
            }]
        );
        assert_eq!(c.items(), &[refined("first refined")]);
    }

    #[test]
    fn refine_only_removes_drafts_not_prior_refined_items() {
        // A prior refined (non-draft) item must NOT be removed by a later refine —
        // only drafts are superseded.
        let mut c = DualCoordinator::new();
        c.on_sensevoice_final("kept refined");
        c.on_moonshine_final("d1");
        c.on_moonshine_final("d2");
        // drafts at indices 1,2 (index 0 is the kept refined) → remove index 1,
        // keep index 2.
        let edits = c.on_sensevoice_final("next refined");
        assert_eq!(
            edits,
            vec![
                ListEdit::Remove { index: 1 },
                ListEdit::Append {
                    item: refined("next refined")
                },
            ]
        );
        assert_eq!(
            c.items(),
            &[
                refined("kept refined"),
                draft("d2"),
                refined("next refined")
            ]
        );
    }

    #[test]
    fn reset_empties_the_list() {
        let mut c = DualCoordinator::new();
        c.on_moonshine_final("x");
        c.on_sensevoice_final("y");
        c.reset();
        assert!(c.items().is_empty());
    }

    #[test]
    fn list_edit_serializes_as_discriminated_union() {
        let a = ListEdit::Append {
            item: refined("hi"),
        };
        let j = serde_json::to_value(&a).unwrap();
        assert_eq!(j["op"], "append");
        assert_eq!(j["item"]["text"], "hi");
        assert_eq!(j["item"]["draft"], false);
        let back: ListEdit = serde_json::from_value(j).unwrap();
        assert_eq!(back, a);

        let r = ListEdit::Remove { index: 2 };
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["op"], "remove");
        assert_eq!(j["index"], 2);
    }
}
