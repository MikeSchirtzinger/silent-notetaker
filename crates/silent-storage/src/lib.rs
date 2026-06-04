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
#![forbid(unsafe_code)]

pub mod backup;
pub mod error;

pub use backup::{Backup, BackupScreenshot, base64_encode};
pub use error::{Result, StorageError};

// Re-export the core storage contract so callers can `use silent_storage::Meeting`
// etc. without depending on silent-core directly.
pub use silent_core::storage::{
    DEXIE_SCHEMA_VERSION, DEXIE_VERSION_MULTIPLIER, ImageEncoding, Meeting, MigrationStatus, Note,
    Screenshot, StorageCounts, StorageEvent, StorageSnapshot, TranscriptChunk,
    expected_idb_version,
};

#[cfg(target_arch = "wasm32")]
pub mod migrate;
#[cfg(target_arch = "wasm32")]
pub mod reader;

#[cfg(target_arch = "wasm32")]
pub use migrate::run_migration;
#[cfg(target_arch = "wasm32")]
pub use reader::{DB_NAME, read_database};

// ---------------------------------------------------------------------------
// wasm-bindgen public API — the surface the JS wiring layer (capture.js /
// migration helper) calls. Wiring happens later, serially; this is the contract
// it binds to. The events are delivered to a JS callback so the UI renders them.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod wasm_api {
    use silent_core::storage::StorageEvent;
    use wasm_bindgen::prelude::*;

    /// Run the Dexie v2 → Rust zero-loss migration.
    ///
    /// `on_event` is invoked with a JSON string for each
    /// [`StorageEvent`] (`{ "tag": ..., "payload": ... }`) — including the
    /// `backup_ready` event the UI uses to offer a backup download BEFORE any
    /// write. Resolves to the after-migration row counts as a JS object, or
    /// rejects with a string error.
    ///
    /// # Errors
    ///
    /// Rejects with a string if the migration fails at any step (the DB is left
    /// at its pre-migration state past the backup point).
    #[wasm_bindgen]
    pub async fn migrate_database(on_event: js_sys::Function) -> Result<JsValue, JsValue> {
        console_error_panic_hook::set_once();

        let emit = move |ev: StorageEvent| {
            // Serialize each event to JSON for the UI. A serialization failure
            // here is non-fatal to the migration (the event is dropped, logged),
            // because losing a progress event must not abort a working migration.
            match serde_json::to_string(&ev) {
                Ok(json) => {
                    let _ = on_event.call1(&JsValue::NULL, &JsValue::from_str(&json));
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[silent-storage] event serialize failed: {e}"
                    )));
                }
            }
        };

        let counts = crate::migrate::run_migration(&emit)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        serde_wasm_bindgen::to_value(&counts)
            .map_err(|e| JsValue::from_str(&format!("counts serialize: {e}")))
    }

    /// Read all four tables and return a JS summary object for verification
    /// (counts + per-table data + screenshot encodings). Used by the browser
    /// test harness and the wiring layer's smoke check.
    ///
    /// # Errors
    ///
    /// Rejects with a string if the IndexedDB read fails at any step.
    #[wasm_bindgen]
    pub async fn read_database_summary() -> Result<JsValue, JsValue> {
        console_error_panic_hook::set_once();

        let snapshot = crate::reader::read_database()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        crate::summary::snapshot_to_summary(&snapshot)
            .map_err(|e| JsValue::from_str(&format!("summary: {e:?}")))
    }
}

#[cfg(target_arch = "wasm32")]
pub mod summary;
