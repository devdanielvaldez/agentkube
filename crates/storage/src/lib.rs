//! Persistence contracts and deterministic in-memory repositories.
//!
//! This crate defines infrastructure-independent repository ports. Database
//! adapters can implement the same contracts without leaking database details
//! into AgentKube's domain crates.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod entity;
mod error;
mod key;
mod resource;

pub use entity::{
    EntityFuture, EntityRepository, EntityRepositoryError, InMemoryEntityRepository,
    VersionedEntity,
};
pub use error::{StorageError, StorageFuture, StorageResult};
pub use key::ResourceKey;
pub use resource::{InMemoryResourceRepository, ResourceRepository};
