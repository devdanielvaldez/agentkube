use std::{error::Error, fmt, io, path::PathBuf};

/// Failure encountered while reading, decoding, or validating configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// A configuration file could not be read.
    Io {
        /// Source that attempted the read.
        source_name: String,
        /// File involved in the failure.
        path: PathBuf,
        /// Underlying I/O failure.
        source: io::Error,
    },
    /// A serialized configuration source could not be decoded.
    Decode {
        /// Human-readable source name.
        source_name: String,
        /// Underlying JSON decoding failure.
        source: serde_json::Error,
    },
    /// A source contained a malformed or unknown value.
    InvalidValue {
        /// Human-readable source name.
        source_name: String,
        /// Key associated with the value.
        key: String,
        /// Rejected value.
        value: String,
        /// Explanation of the expected format.
        message: String,
    },
    /// The merged configuration violates a cross-field invariant.
    Validation(ConfigValidationError),
}

impl ConfigError {
    pub(crate) fn invalid_value(
        source_name: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::InvalidValue {
            source_name: source_name.into(),
            key: key.into(),
            value: value.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                source_name,
                path,
                source,
            } => write!(
                formatter,
                "configuration source {source_name:?} could not read {}: {source}",
                path.display()
            ),
            Self::Decode {
                source_name,
                source,
            } => write!(
                formatter,
                "configuration source {source_name:?} could not be decoded: {source}"
            ),
            Self::InvalidValue {
                source_name,
                key,
                value,
                message,
            } => write!(
                formatter,
                "configuration source {source_name:?} has invalid {key} value {value:?}: {message}"
            ),
            Self::Validation(error) => error.fmt(formatter),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            Self::Validation(source) => Some(source),
            Self::InvalidValue { .. } => None,
        }
    }
}

/// A cross-field configuration invariant that was violated after merging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    field: &'static str,
    message: &'static str,
}

impl ConfigValidationError {
    pub(crate) const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }

    /// Returns the dotted path of the invalid field.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the violated invariant.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid configuration at {}: {}",
            self.field, self.message
        )
    }
}

impl Error for ConfigValidationError {}
