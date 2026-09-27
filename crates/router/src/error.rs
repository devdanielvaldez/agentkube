use agentkube_agents::{ModelName, ProviderName};
use agentkube_providers::ProviderError;
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Result returned by routing operations.
pub type RouterResult<T> = Result<T, RouterError>;

/// Sendable future returned by routing operations.
pub type RouterFuture<'a, T> = Pin<Box<dyn Future<Output = RouterResult<T>> + Send + 'a>>;

/// Invalid provider registration or registry operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// Another adapter already owns this provider name.
    DuplicateProvider(ProviderName),
    /// A routing profile appears more than once.
    DuplicateProfile(ModelName),
    /// The provider advertises a model with no routing profile.
    MissingProfile(ModelName),
    /// A profile refers to a model absent from the provider catalog.
    UnknownModel(ModelName),
    /// The registry lock is unavailable.
    Unavailable(&'static str),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider(provider) => {
                write!(formatter, "provider {provider} is already registered")
            }
            Self::DuplicateProfile(model) => write!(formatter, "duplicate profile for {model}"),
            Self::MissingProfile(model) => write!(formatter, "missing routing profile for {model}"),
            Self::UnknownModel(model) => {
                write!(formatter, "profile references unknown model {model}")
            }
            Self::Unavailable(reason) => {
                write!(formatter, "provider registry unavailable: {reason}")
            }
        }
    }
}

impl Error for RegistryError {}

/// Failure to select or invoke an eligible model route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterError {
    /// Fixed routing names a provider that is not registered.
    ProviderNotRegistered(ProviderName),
    /// Fixed routing names a model absent from that provider.
    ModelNotRegistered {
        /// Selected provider.
        provider: ProviderName,
        /// Selected model.
        model: ModelName,
    },
    /// No registered model satisfies health, policy, capability, and budget filters.
    NoEligibleModels {
        /// Health or estimation failures encountered while evaluating candidates.
        provider_errors: Vec<ProviderError>,
    },
    /// A non-retryable provider failure stopped failover.
    ProviderRejected {
        /// Terminal non-retryable error.
        error: ProviderError,
        /// Retryable failures encountered before the terminal rejection.
        prior_failures: Vec<ProviderError>,
    },
    /// Every eligible route failed with a retryable error.
    AllProvidersFailed(Vec<ProviderError>),
    /// Registry access failed.
    Registry(RegistryError),
}

impl fmt::Display for RouterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderNotRegistered(provider) => {
                write!(formatter, "provider {provider} is not registered")
            }
            Self::ModelNotRegistered { provider, model } => {
                write!(formatter, "model {model} is not registered for {provider}")
            }
            Self::NoEligibleModels { provider_errors } => write!(
                formatter,
                "no model satisfies routing constraints ({} provider errors)",
                provider_errors.len()
            ),
            Self::ProviderRejected {
                error,
                prior_failures,
            } => write!(
                formatter,
                "provider rejected route after {} failed attempts: {error}",
                prior_failures.len()
            ),
            Self::AllProvidersFailed(errors) => {
                write!(formatter, "all {} eligible routes failed", errors.len())
            }
            Self::Registry(error) => error.fmt(formatter),
        }
    }
}

impl Error for RouterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ProviderRejected { error, .. } => Some(error),
            Self::Registry(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RegistryError> for RouterError {
    fn from(value: RegistryError) -> Self {
        Self::Registry(value)
    }
}
