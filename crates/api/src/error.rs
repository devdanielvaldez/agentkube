use agentkube_protocol::{ApiError, ApiErrorReason, ApiStatusCode};
use agentkube_queue::QueueError;
use agentkube_storage::StorageError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::{error::Error, fmt};

/// HTTP-aware wrapper around AgentKube's stable API error document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpApiError(ApiError);

impl HttpApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, ApiErrorReason::BadRequest, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, ApiErrorReason::NotFound, message)
    }

    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(401, ApiErrorReason::Unauthorized, message)
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, ApiErrorReason::Conflict, message)
    }

    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self::new(422, ApiErrorReason::ValidationFailed, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(500, ApiErrorReason::Internal, message)
    }

    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::new(503, ApiErrorReason::Internal, message)
    }

    fn new(code: u16, reason: ApiErrorReason, message: impl Into<String>) -> Self {
        Self(ApiError::new(
            ApiStatusCode::new(code).expect("HTTP API errors always use 4xx or 5xx status codes"),
            reason,
            message,
        ))
    }

    /// Returns the stable error response body.
    #[must_use]
    pub const fn api_error(&self) -> &ApiError {
        &self.0
    }
}

impl fmt::Display for HttpApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for HttpApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

impl IntoResponse for HttpApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code().get())
            .expect("validated API status code is a valid HTTP status");
        (status, Json(self.0)).into_response()
    }
}

impl From<StorageError> for HttpApiError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::AlreadyExists(_)
            | StorageError::UidAlreadyExists(_)
            | StorageError::Conflict { .. }
            | StorageError::IdentityChanged { .. } => Self::conflict(error.to_string()),
            StorageError::NotFound(_) => Self::not_found(error.to_string()),
            StorageError::InvalidInitialVersion { .. } => Self::validation(error.to_string()),
            StorageError::VersionExhausted(_) => Self::internal(error.to_string()),
            StorageError::Unavailable(_) => Self::unavailable(error.to_string()),
        }
    }
}

impl From<QueueError> for HttpApiError {
    fn from(error: QueueError) -> Self {
        match error {
            QueueError::AlreadyQueued(_) | QueueError::LeaseOwnerMismatch { .. } => {
                Self::conflict(error.to_string())
            }
            QueueError::LeaseNotFound(_) => Self::not_found(error.to_string()),
            QueueError::DeliveryAttemptsExhausted(_)
            | QueueError::SequenceExhausted
            | QueueError::TimeOverflow => Self::internal(error.to_string()),
            QueueError::Unavailable(_) => Self::unavailable(error.to_string()),
        }
    }
}
