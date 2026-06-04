//! Browser storage and Dexie v2 migration (stub — Phase 4).
//!
//! Will house IndexedDB access via `indexed_db_futures` and the zero-loss
//! migration of the existing Dexie v2 `SilentNotetaker` database
//! (meetings, transcriptChunks, notes, screenshots — including binary
//! screenshot blobs), productionized from `docs/research/spike-storage.md`.
//! Existing users' meetings must survive the upgrade; an export-backup is
//! offered before migration (PRD Phase 4 exit criterion, R4 cache policy).
//!
//! The IndexedDB/migration code (Task H2) is built separately; this file
//! currently homes only the pure [`search`] module (Task H3): meeting-history
//! last-50 listing and title/notes/transcript search, ported byte-identically
//! from `index.html` and testable without a browser.
#![forbid(unsafe_code)]

pub mod search;
