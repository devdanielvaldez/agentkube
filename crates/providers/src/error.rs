use agentkube_agents::ProviderName;
use agentkube_core::HumanDuration;
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Result returned by model-provider operations.
pub type ProviderResult<T> = Result<T, ProviderError>;

/// Sendable future returned by model-provider operations.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

/// Normalized failure category shared by all provider adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// Credentials are missing or rejected.
    Authentication,
    /// The provider denied the request due to a rate limit.
    RateLimited {
        /// Provider-advertised retry delay, when available.
        retry_after: Option<HumanDuration>,
    },
    /// The provider rejected the request shape or parameter values.
    InvalidRequest,
    /// The requested model is not known to the provider.
    ModelNotFound,
    /// The input exceeds the model context window.
    ContextLengthExceeded,
    /// Provider safety systems refused the content.
    ContentFiltered,
    /// The provider call exceeded its deadline.
    Timeout,
    /// The provider is temporarily unable to serve requests.
    Unavailable,
    /// The provider returned an invalid or unsupported wire response.
    Protocol,
    /// The adapter failed for an unexpected internal reason.
    Internal,
}

impl ProviderErrorKind {
    /// Returns whether retry or failover may succeed without changing the request.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Timeout | Self::Unavailable | Self::Internal
        )
    }
}

/// Provider failure carrying normalized routing semantics and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    provider: ProviderName,
    kind: ProviderErrorKind,
    message: String,
}

impl ProviderError {
    /// Creates a normalized provider error.
    #[must_use]
    pub fn new(
        provider: ProviderName,
        kind: ProviderErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            kind,
            message: message.into(),
        }
    }

    /// Returns the provider that produced the failure.
    #[must_use]
    pub const fn provider(&self) -> &ProviderName {
        &self.provider
    }

    /// Returns the normalized failure category.
    #[must_use]
    pub const fn kind(&self) -> ProviderErrorKind {
        self.kind
    }

    /// Returns the adapter or provider diagnostic.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns whether retry or failover may succeed unchanged.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider {} returned {:?}: {}",
            self.provider, self.kind, self.message
        )
    }
}

impl Error for ProviderError {}
