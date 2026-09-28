//! Typed operator failures with process-level consequences.

use std::{error::Error, fmt};

/// Failure produced while building or running the operator.
#[derive(Debug)]
pub enum OperatorError {
    /// Durable storage could not be opened.
    Storage(String),
    /// A repository operation failed unexpectedly.
    Repository(String),
    /// The dispatch queue is unavailable.
    Queue(String),
    /// Provider registry, catalog, or discovery failed.
    Providers(String),
    /// A scheduling or routing invariant was violated.
    Scheduling(String),
    /// A reconciliation cycle failed unexpectedly.
    Reconcile(String),
    /// A worker lifecycle operation failed unexpectedly.
    Worker(String),
    /// Configuration is missing a required value.
    Config(String),
}

impl OperatorError {
    fn message(&self) -> &str {
        match self {
            Self::Storage(message)
            | Self::Repository(message)
            | Self::Queue(message)
            | Self::Providers(message)
            | Self::Scheduling(message)
            | Self::Reconcile(message)
            | Self::Worker(message)
            | Self::Config(message) => message,
        }
    }
}

impl fmt::Display for OperatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "operator failed: {}", self.message())
    }
}

impl Error for OperatorError {}

impl From<agentkube_storage::StorageError> for OperatorError {
    fn from(error: agentkube_storage::StorageError) -> Self {
        Self::Repository(error.to_string())
    }
}

impl From<agentkube_queue::QueueError> for OperatorError {
    fn from(error: agentkube_queue::QueueError) -> Self {
        Self::Queue(error.to_string())
    }
}

impl From<agentkube_providers_http::AdapterError> for OperatorError {
    fn from(error: agentkube_providers_http::AdapterError) -> Self {
        Self::Providers(error.to_string())
    }
}

impl From<agentkube_router::RegistryError> for OperatorError {
    fn from(error: agentkube_router::RegistryError) -> Self {
        Self::Providers(error.to_string())
    }
}

impl From<reqwest::Error> for OperatorError {
    fn from(error: reqwest::Error) -> Self {
        Self::Providers(error.to_string())
    }
}
