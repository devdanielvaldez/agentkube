//! Typed CLI errors with stable exit codes.
//!
//! All user-facing failures flow through [`CliError`]. Diagnostics include the
//! operation, HTTP status and reason, and the server message without dumping
//! oversized bodies or secrets.

use agentkube_protocol::ApiError;
use std::{fmt, io};

/// Stable process exit codes.
pub const EXIT_SUCCESS: i32 = 0;
/// Transport, server, or unexpected runtime failure.
pub const EXIT_RUNTIME: i32 = 1;
/// CLI usage, local file, decoding, or client-side validation failure.
pub const EXIT_USAGE: i32 = 2;
/// Partial multi-document apply.
pub const EXIT_PARTIAL: i32 = 3;

/// Single CLI error type preserving context and optional server details.
#[derive(Debug)]
pub enum CliError {
    /// CLI usage error (invalid flags, URL, timeout, pagination, replicas).
    Usage(String),
    /// Local file, decoding, or client-side domain validation failure.
    InvalidInput(String),
    /// Transport or unexpected runtime failure.
    Transport {
        /// Stable operation name such as `get agent "foo"`.
        operation: String,
        /// Human-readable cause without secrets.
        message: String,
    },
    /// Structured server error preserving the wire [`ApiError`].
    Api {
        /// Stable operation name.
        operation: String,
        /// HTTP status code.
        status: u16,
        /// Server-provided structured error.
        error: Box<ApiError>,
    },
    /// Multi-document apply stopped after partial success.
    Partial {
        /// Number of documents applied before the failure.
        applied: usize,
        /// Total documents in the input stream.
        total: usize,
        /// Cause of the failure.
        message: String,
    },
    /// Output stream closed by the reader (for example piped to `head`).
    BrokenPipe,
}

impl CliError {
    /// Creates a usage error.
    #[must_use]
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    /// Creates a local input or client-validation error.
    #[must_use]
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    /// Creates a transport error with operation context.
    #[must_use]
    pub fn transport(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Transport {
            operation: operation.into(),
            message: message.into(),
        }
    }

    /// Creates a structured API error.
    #[must_use]
    pub fn api(operation: impl Into<String>, status: u16, error: ApiError) -> Self {
        Self::Api {
            operation: operation.into(),
            status,
            error: Box::new(error),
        }
    }

    /// Creates a partial-apply error.
    #[must_use]
    pub fn partial(applied: usize, total: usize, message: impl Into<String>) -> Self {
        Self::Partial {
            applied,
            total,
            message: message.into(),
        }
    }

    /// Returns the stable process exit code for this error.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::InvalidInput(_) => EXIT_USAGE,
            Self::Partial { .. } => EXIT_PARTIAL,
            Self::BrokenPipe => EXIT_SUCCESS,
            Self::Transport { .. } | Self::Api { .. } => EXIT_RUNTIME,
        }
    }

    /// Returns the preserved server error when one exists.
    #[must_use]
    pub fn api_error(&self) -> Option<&ApiError> {
        match self {
            Self::Api { error, .. } => Some(error),
            _ => None,
        }
    }

    /// Returns true for clean broken-pipe termination.
    #[must_use]
    pub const fn is_broken_pipe(&self) -> bool {
        matches!(self, Self::BrokenPipe)
    }

    /// Maps an I/O error on stdout to broken-pipe or transport.
    #[must_use]
    pub fn from_stdout_io(operation: &str, error: &io::Error) -> Self {
        if error.kind() == io::ErrorKind::BrokenPipe {
            Self::BrokenPipe
        } else {
            Self::transport(operation, format!("failed to write output: {error}"))
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(formatter, "usage error: {message}"),
            Self::InvalidInput(message) => write!(formatter, "invalid input: {message}"),
            Self::Transport { operation, message } => {
                write!(formatter, "error during {operation}: {message}")
            }
            Self::Api {
                operation,
                status,
                error,
            } => {
                write!(
                    formatter,
                    "error during {operation}: HTTP {status} {}: {}",
                    error.reason_string(),
                    error.message()
                )
            }
            Self::Partial {
                applied,
                total,
                message,
            } => write!(
                formatter,
                "partial apply: {applied} of {total} documents applied before failure: {message}"
            ),
            Self::BrokenPipe => formatter.write_str("output pipe closed"),
        }
    }
}

impl std::error::Error for CliError {}

/// Extension to render [`ApiErrorReason`] without importing it everywhere.
trait ReasonString {
    /// Returns the stable SCREAMING_SNAKE_CASE reason.
    fn reason_string(&self) -> String;
}

impl ReasonString for ApiError {
    fn reason_string(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.get("reason").cloned())
            .and_then(|reason| reason.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| "ERROR".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentkube_protocol::{ApiErrorReason, ApiStatusCode};

    fn sample_api_error() -> ApiError {
        ApiError::new(
            ApiStatusCode::new(404).unwrap(),
            ApiErrorReason::NotFound,
            "agent was not found",
        )
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(CliError::usage("x").exit_code(), 2);
        assert_eq!(CliError::invalid_input("x").exit_code(), 2);
        assert_eq!(
            CliError::transport("op", "down").exit_code(),
            1,
            "transport failures must exit 1"
        );
        assert_eq!(CliError::api("op", 404, sample_api_error()).exit_code(), 1);
        assert_eq!(CliError::partial(1, 2, "boom").exit_code(), 3);
        assert_eq!(CliError::BrokenPipe.exit_code(), 0);
    }

    #[test]
    fn api_diagnostics_include_operation_status_and_message() {
        let error = CliError::api("get agent \"foo\"", 404, sample_api_error());
        let rendered = error.to_string();

        assert!(rendered.contains("get agent"), "{rendered}");
        assert!(rendered.contains("404"), "{rendered}");
        assert!(rendered.contains("was not found"), "{rendered}");
    }

    #[test]
    fn broken_pipe_is_detected_from_io_errors() {
        let pipe = io::Error::new(io::ErrorKind::BrokenPipe, "closed");
        assert!(CliError::from_stdout_io("write", &pipe).is_broken_pipe());
    }
}
