//! History, identities and the small JSON files of the meter, in the same directory, SQLite schema and file formats as
//! the C# server, so an existing user's data opens unchanged.
//!
//! Snapshots are `serde_json::Value` until meter-core's typed snapshots exist; every write takes `impl Serialize` and
//! every typed read `DeserializeOwned`, so switching is a change of type parameters, not of the API.

mod archive;
mod capture_history;
mod character_context;
mod encounters;
mod history;
mod identity;
pub mod json;
pub mod paths;
pub mod time;

pub use archive::{HistoryArchive, HistoryImportResult, MAX_UPLOAD_BYTES};
pub use capture_history::{local_capture_files, CaptureSettings};
pub use character_context::{CharacterContextStore, KnownPlayers};
pub use encounters::{DungeonSummary, EncounterStore, EncounterSummary, LISTED_ENCOUNTERS, RETAINED_DUNGEONS, RETAINED_ENCOUNTERS};
pub use identity::{IdentityResolution, IdentitySighting, IdentityTimelineEntry, KnownCharacter, UnidentifiedPlayer};
pub use time::DateTimeOffset;

use std::fmt;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    /// C# `InvalidDataException`: a history file or record that is not accepted; the message is shown to the user.
    Invalid(String),
    /// C# `KeyNotFoundException`.
    NotFound(String),
    /// C# `ArgumentException`.
    Argument(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
            Self::Invalid(message) | Self::NotFound(message) | Self::Argument(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
