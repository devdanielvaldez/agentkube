//! Typed errors for opening and initializing SQLite storage.

use std::{error::Error, fmt, path::PathBuf};

/// Failure produced while opening or initializing a SQLite store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteError {
    operation: &'static str,
    message: String,
}

impl SqliteError {
    /// Records a failure to open the database file.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            operation: "open",
            message: format!("{}: {}", path.into().display(), message.into()),
        }
    }

    /// Records a failure while creating the schema or pragmas.
    #[must_use]
    pub fn schema(message: impl Into<String>) -> Self {
        Self {
            operation: "schema",
            message: message.into(),
        }
    }

    /// Returns the operation that failed (`open` or `schema`).
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        self.operation
    }

    /// Returns the human-readable cause.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SqliteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "sqlite {} failed: {}",
            self.operation, self.message
        )
    }
}

impl Error for SqliteError {}
