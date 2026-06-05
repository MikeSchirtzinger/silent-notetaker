//! Browser storage and the zero-loss Dexie v2 migration (PRD Phase 4, Task H2).
//!
//! Productionizes the S4 spike (`docs/research/spike-storage.md`). The typed
//! record + boundary-event SHAPES live in [`silent_core::storage`] (browser-free,
//! the contract); this crate is the BROWSER RUNTIME that opens the real Dexie v2
//! `SilentNotetaker` IndexedDB, reads all four tables, and migrates it.
//!
//! # What is here
//!
//! - [`backup`] — the export-backup serializer (pure Rust, native-testable): a
//!   self-contained JSON document with all four tables, screenshot bytes
//!   base64-encoded. Produced and offered to the user BEFORE any migration write
//!   (PRD Phase 4 exit criterion / Key Risk: "export-backup before migrate").
//! - [`reader`] (wasm32) — the PROVEN two-phase reader: opens the DB version-less
//!   (the Dexie ×10 rule) and resolves all three screenshot encodings (base64
//!   data-URL string = the shipping format, JS `Blob`, `Uint8Array`) with no IDB
//!   transaction held across a non-IDB `await`.
//! - [`migrate`] (wasm32) — the migration: backup → normalize screenshots to
//!   `Uint8Array` → verify zero loss → record completion. Emits
//!   [`silent_core::storage::StorageEvent`]s through a caller-supplied callback.
//!
//! # Native vs wasm
//!
//! The reader/migrator are `cfg(target_arch = "wasm32")`-gated because they need
//! `indexed_db_futures` / `web-sys`. The native build still compiles this crate
//! (so `cargo check --workspace` works), re-exports the core record types, and
//! exercises the pure-Rust [`backup`] logic under native unit tests. The browser
//! read/migrate round-trip is exercised by `wasm-bindgen-test`
//! (`tests/browser_idb.rs`), run at wiring time against a live IndexedDB.
//!
//! - [`search`] — the meeting-history last-50 listing and title/notes/transcript
//!   search (Task H3): pure, browser-free policy ported byte-identically from
//!   `index.html`.
#![forbid(unsafe_code)]

pub mod backup;
pub mod error;
pub mod search;

pub use backup::{Backup, BackupScreenshot, base64_encode};
pub use error::{Result, StorageError};

// Re-export the core storage contract so callers can `use silent_storage::Meeting`
// etc. without depending on silent-core directly.
pub use silent_core::storage::{
    DEXIE_SCHEMA_VERSION, DEXIE_VERSION_MULTIPLIER, ImageEncoding, Meeting, MigrationStatus, Note,
    RUST_SCHEMA_VERSION, SPEAKER_NAMES_STORE, Screenshot, SpeakerName, StorageCounts, StorageEvent,
    StorageSnapshot, TranscriptChunk, expected_idb_version,
};

#[cfg(target_arch = "wasm32")]
pub mod migrate;
#[cfg(target_arch = "wasm32")]
pub mod reader;
#[cfg(target_arch = "wasm32")]
pub mod writer;

#[cfg(target_arch = "wasm32")]
pub use migrate::run_migration;
#[cfg(target_arch = "wasm32")]
pub use reader::{DB_NAME, read_database};

// ---------------------------------------------------------------------------
// wasm-bindgen surface NOTE
//
// The browser-facing `#[wasm_bindgen]` surface (migrate_database, the live CRUD,
// recent_meetings/meeting_detail, the durable speaker-name map) lives in
// `silent-web`'s `storage` module, NOT here. `silent-web` links this crate and
// pulls those exports into the single shared `silent_web.js` pkg the app loads
// (one wasm binary, many surfaces — the same pattern as session/notes/diarization).
//
// Declaring the `#[wasm_bindgen]` functions here too would emit duplicate exports
// into `silent_web.js` when `silent-web` links this crate. So this crate stays a
// pure library: it owns the POLICY (IDB access, schema v3, the zero-loss
// migration, the export-backup); `silent-web` owns the thin glue.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub mod summary;
