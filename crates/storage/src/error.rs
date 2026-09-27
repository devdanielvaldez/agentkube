use crate::ResourceKey;
use agentkube_core::{ResourceUid, ResourceVersion};
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Result returned by declarative resource repositories.
pub type StorageResult<T> = Result<T, StorageError>;

/// Sendable future returned by declarative resource repository operations.
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = StorageResult<T>> + Send + 'a>>;

/// Failure produced while persisting a declarative resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// A resource already occupies the requested namespaced key.
    AlreadyExists(ResourceKey),
    /// A different resource already owns the supplied stable UID.
    UidAlreadyExists(ResourceUid),
    /// No resource exists at the requested key.
    NotFound(ResourceKey),
    /// The caller attempted to replace a resource using a stale version.
    Conflict {
        /// Key of the resource being replaced.
        key: ResourceKey,
        /// Version supplied by the caller.
        expected: ResourceVersion,
        /// Version currently stored.
        actual: ResourceVersion,
    },
    /// A replacement attempted to change a resource's immutable UID.
    IdentityChanged {
        /// Key of the resource being replaced.
        key: ResourceKey,
        /// UID currently stored.
        expected: ResourceUid,
        /// UID supplied by the caller.
        actual: ResourceUid,
    },
    /// A newly created resource did not start at version one.
    InvalidInitialVersion {
        /// Key of the invalid resource.
        key: ResourceKey,
        /// Version supplied by the caller.
        actual: ResourceVersion,
    },
    /// The resource version reached its maximum value.
    VersionExhausted(ResourceKey),
    /// The repository backend could not complete an operation.
    Unavailable(&'static str),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists(key) => write!(formatter, "resource {key} already exists"),
            Self::UidAlreadyExists(uid) => write!(formatter, "resource UID {uid} already exists"),
            Self::NotFound(key) => write!(formatter, "resource {key} was not found"),
            Self::Conflict {
                key,
                expected,
                actual,
            } => write!(
                formatter,
                "resource {key} has version {}, not expected version {}",
                actual.get(),
                expected.get()
            ),
            Self::IdentityChanged {
                key,
                expected,
                actual,
            } => write!(
                formatter,
                "resource {key} cannot change UID from {expected} to {actual}"
            ),
            Self::InvalidInitialVersion { key, actual } => write!(
                formatter,
                "resource {key} must be created at version 1, received {}",
                actual.get()
            ),
            Self::VersionExhausted(key) => {
                write!(formatter, "resource {key} version counter is exhausted")
            }
            Self::Unavailable(reason) => write!(formatter, "storage is unavailable: {reason}"),
        }
    }
}

impl Error for StorageError {}
