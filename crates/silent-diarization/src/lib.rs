//! Speaker diarization (stub — Phase 2).
//!
//! Will house the TitaNet-small embedder on `rust-ort-web` (productionized from
//! `docs/research/spike-titanet.md`, cosine 1.000000 against the `eval/`
//! fixtures), the SpeakerTracker port (centroid clustering, 8-color rotation,
//! thresholds as config), the **stop-time global recluster** from
//! `docs/DIARIZATION.md` (a new capability), and rename / merge-by-rename as
//! Rust policy that survives reclustering. Raw embeddings never cross an
//! extension or network boundary (PRD R5/R7).
//!
//! Empty by design (Task C1 scaffold).
#![forbid(unsafe_code)]
