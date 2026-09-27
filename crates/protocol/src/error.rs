use crate::ResourceKind;
use agentkube_core::ResourceName;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt};

/// A validated HTTP client or server error status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ApiStatusCode(u16);

impl ApiStatusCode {
    /// Creates an API status code in the inclusive `400..=599` range.
    pub const fn new(value: u16) -> Result<Self, InvalidStatusCode> {
        if value >= 400 && value <= 599 {
            Ok(Self(value))
        } else {
            Err(InvalidStatusCode(value))
        }
    }

    /// Returns the numeric status code.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ApiStatusCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Error returned for a status code outside the API error range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidStatusCode(u16);

impl InvalidStatusCode {
    /// Returns the rejected status code.
    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for InvalidStatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "status code {} is outside 400..=599", self.0)
    }
}

impl Error for InvalidStatusCode {}

/// Stable machine-readable reason for an API failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ApiErrorReason {
    /// The request is malformed.
    BadRequest,
    /// Authentication is required or invalid.
    Unauthorized,
    /// The caller lacks permission for the operation.
    Forbidden,
    /// The requested resource does not exist.
    NotFound,
    /// The request conflicts with current resource state.
    Conflict,
    /// One or more fields failed domain validation.
    ValidationFailed,
    /// A rate limit prevents immediate execution.
    RateLimited,
    /// A configured financial or token budget was exhausted.
    BudgetExceeded,
    /// A required model provider is unavailable.
    ProviderUnavailable,
    /// An unexpected server failure occurred.
    Internal,
}

/// Structured API error suitable for HTTP and watch responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    code: ApiStatusCode,
    reason: ApiErrorReason,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<ApiErrorDetails>,
}

impl ApiError {
    /// Creates a structured API error.
    #[must_use]
    pub fn new(code: ApiStatusCode, reason: ApiErrorReason, message: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            message: message.into(),
            details: None,
        }
    }

    /// Attaches structured diagnostic details.
    #[must_use]
    pub fn with_details(mut self, details: ApiErrorDetails) -> Self {
        self.details = Some(details);
        self
    }

    /// Returns the HTTP-compatible error status code.
    #[must_use]
    pub const fn code(&self) -> ApiStatusCode {
        self.code
    }

    /// Returns the stable machine-readable reason.
    #[must_use]
    pub const fn reason(&self) -> ApiErrorReason {
        self.reason
    }

    /// Returns the human-readable error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns optional structured details.
    #[must_use]
    pub const fn details(&self) -> Option<&ApiErrorDetails> {
        self.details.as_ref()
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "API error {}: {}", self.code.get(), self.message)
    }
}

impl Error for ApiError {}

/// Optional resource and field context for an [`ApiError`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorDetails {
    /// Kind of resource involved in the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ResourceKind>,
    /// Name of the resource involved in the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ResourceName>,
    /// Per-field validation failures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<FieldViolation>,
    /// Delay suggested before retrying the operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u32>,
}

/// Identifies a field that failed request validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldViolation {
    field: String,
    message: String,
}

impl FieldViolation {
    /// Creates a field validation failure.
    #[must_use]
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Returns the field path.
    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Returns the validation message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_success_status_codes() {
        assert_eq!(ApiStatusCode::new(200), Err(InvalidStatusCode(200)));
    }

    #[test]
    fn deserialization_cannot_bypass_status_validation() {
        assert!(serde_json::from_str::<ApiStatusCode>("999").is_err());
    }

    #[test]
    fn error_serializes_with_stable_reason() {
        let error = ApiError::new(
            ApiStatusCode::new(429).unwrap(),
            ApiErrorReason::RateLimited,
            "provider rate limit reached",
        );
        let value = serde_json::to_value(error).unwrap();

        assert_eq!(value["code"], 429);
        assert_eq!(value["reason"], "RATE_LIMITED");
        assert!(value.get("details").is_none());
    }
}
