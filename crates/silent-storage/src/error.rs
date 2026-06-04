//! Error type for `silent-storage` IndexedDB access and migration.

use thiserror::Error;

/// Errors from opening, reading, backing up, or migrating the `SilentNotetaker`
/// database.
///
/// `#[non_exhaustive]`: new failure modes can be added without a breaking change;
/// callers must include a wildcard arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// Opening the IndexedDB database failed (or it does not exist yet).
    #[error("IDB open error: {0}")]
    Open(String),

    /// A transaction or object-store operation failed.
    #[error("IDB operation error: {0}")]
    Operation(String),

    /// A record could not be deserialized into its typed struct (a missing or
    /// mistyped field). The migration treats this as a hard error rather than
    /// silently dropping the row — losing a row is the exact failure this whole
    /// subsystem exists to prevent.
    #[error("deserialize error: {0}")]
    Deserialize(String),

    /// A JS exception crossed the boundary (`DomException` or generic `JsValue`).
    #[error("JS exception: {0}")]
    Js(String),

    /// The database was at an unexpected IndexedDB version (the Dexie ×10
    /// pre-flight check, [`silent_core::storage::expected_idb_version`]). The
    /// migration refuses to touch a DB it does not recognize.
    #[error("unexpected IDB version: expected {expected}, found {actual}")]
    UnexpectedVersion {
        /// The IDB version the migration expects (`dexie_version × 10`).
        expected: u32,
        /// The IDB version actually found.
        actual: u32,
    },

    /// A serialization error while building the export-backup JSON.
    #[error("backup serialization error: {0}")]
    Backup(String),

    /// A zero-loss invariant was violated: the post-migration readback did not
    /// match the pre-migration snapshot. The migration aborts and reports this.
    #[error("zero-loss check failed: {0}")]
    ZeroLoss(String),
}

/// Convenience `Result` alias for this crate.
pub type Result<T> = std::result::Result<T, StorageError>;

#[cfg(target_arch = "wasm32")]
impl From<indexed_db_futures::error::Error> for StorageError {
    fn from(e: indexed_db_futures::error::Error) -> Self {
        StorageError::Operation(format!("{e:?}"))
    }
}

#[cfg(target_arch = "wasm32")]
impl From<wasm_bindgen::JsValue> for StorageError {
    fn from(e: wasm_bindgen::JsValue) -> Self {
        StorageError::Js(format!("{e:?}"))
    }
}
