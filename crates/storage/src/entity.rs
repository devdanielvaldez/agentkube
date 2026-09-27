use agentkube_agents::AgentInstance;
use agentkube_core::AgentId;
use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Debug},
    future::Future,
    hash::Hash,
    pin::Pin,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

type EntityMap<E> = HashMap<<E as VersionedEntity>::Id, E>;
type EntityReadGuard<'a, E> = RwLockReadGuard<'a, EntityMap<E>>;
type EntityWriteGuard<'a, E> = RwLockWriteGuard<'a, EntityMap<E>>;
type EntityResult<T, E> = Result<T, EntityRepositoryError<<E as VersionedEntity>::Id>>;

/// Runtime entity with stable identity and optimistic-concurrency revision.
pub trait VersionedEntity: Clone + Send + Sync + 'static {
    /// Stable identifier type for this entity.
    type Id: Copy + Debug + Eq + Hash + Send + Sync + 'static;

    /// Returns the stable entity identifier.
    fn id(&self) -> Self::Id;

    /// Returns the current entity revision.
    fn revision(&self) -> u64;
}

impl VersionedEntity for AgentInstance {
    type Id = AgentId;

    fn id(&self) -> Self::Id {
        self.id()
    }

    fn revision(&self) -> u64 {
        self.revision()
    }
}

/// Sendable future returned by runtime entity repository operations.
pub type EntityFuture<'a, T, I> =
    Pin<Box<dyn Future<Output = Result<T, EntityRepositoryError<I>>> + Send + 'a>>;

/// Failure produced while persisting a versioned runtime entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityRepositoryError<I> {
    /// An entity already exists with this identifier.
    AlreadyExists(I),
    /// No entity exists with this identifier.
    NotFound(I),
    /// The expected revision is stale.
    Conflict {
        /// Identifier of the entity being mutated.
        id: I,
        /// Revision expected by the caller.
        expected: u64,
        /// Revision currently stored.
        actual: u64,
    },
    /// A new entity did not begin at revision one.
    InvalidInitialRevision {
        /// Identifier of the invalid entity.
        id: I,
        /// Revision supplied by the caller.
        actual: u64,
    },
    /// A replacement did not advance its revision.
    RevisionNotAdvanced {
        /// Identifier of the invalid entity.
        id: I,
        /// Current persisted revision.
        current: u64,
        /// Revision supplied by the replacement.
        supplied: u64,
    },
    /// The repository backend could not complete an operation.
    Unavailable(&'static str),
}

impl<I: Debug> fmt::Display for EntityRepositoryError<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists(id) => write!(formatter, "entity {id:?} already exists"),
            Self::NotFound(id) => write!(formatter, "entity {id:?} was not found"),
            Self::Conflict {
                id,
                expected,
                actual,
            } => write!(
                formatter,
                "entity {id:?} has revision {actual}, not expected revision {expected}"
            ),
            Self::InvalidInitialRevision { id, actual } => write!(
                formatter,
                "entity {id:?} must be inserted at revision 1, received {actual}"
            ),
            Self::RevisionNotAdvanced {
                id,
                current,
                supplied,
            } => write!(
                formatter,
                "entity {id:?} replacement revision {supplied} must exceed {current}"
            ),
            Self::Unavailable(reason) => write!(formatter, "storage is unavailable: {reason}"),
        }
    }
}

impl<I: Debug> Error for EntityRepositoryError<I> {}

/// Asynchronous persistence port for versioned runtime entities.
pub trait EntityRepository<E>: Send + Sync
where
    E: VersionedEntity,
{
    /// Inserts a new entity at revision one.
    fn insert<'a>(&'a self, entity: E) -> EntityFuture<'a, E, E::Id>;

    /// Retrieves an entity by identifier.
    fn get<'a>(&'a self, id: E::Id) -> EntityFuture<'a, Option<E>, E::Id>;

    /// Lists all entities. No ordering is guaranteed.
    fn list<'a>(&'a self) -> EntityFuture<'a, Vec<E>, E::Id>;

    /// Replaces an entity when the expected revision is current.
    ///
    /// The entity itself must already contain a strictly newer revision. This
    /// keeps lifecycle revision ownership in the domain entity.
    fn replace<'a>(&'a self, entity: E, expected_revision: u64) -> EntityFuture<'a, E, E::Id>;

    /// Deletes an entity when the expected revision is current.
    fn delete<'a>(&'a self, id: E::Id, expected_revision: u64) -> EntityFuture<'a, E, E::Id>;
}

/// Thread-safe in-memory implementation of [`EntityRepository`].
pub struct InMemoryEntityRepository<E: VersionedEntity> {
    inner: Arc<RwLock<EntityMap<E>>>,
}

impl<E: VersionedEntity> Clone for InMemoryEntityRepository<E> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<E: VersionedEntity> Default for InMemoryEntityRepository<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: VersionedEntity> InMemoryEntityRepository<E> {
    /// Creates an empty repository.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn read(&self) -> EntityResult<EntityReadGuard<'_, E>, E> {
        self.inner
            .read()
            .map_err(|_| EntityRepositoryError::Unavailable("entity store lock is poisoned"))
    }

    fn write(&self) -> EntityResult<EntityWriteGuard<'_, E>, E> {
        self.inner
            .write()
            .map_err(|_| EntityRepositoryError::Unavailable("entity store lock is poisoned"))
    }
}

impl<E: VersionedEntity> EntityRepository<E> for InMemoryEntityRepository<E> {
    fn insert<'a>(&'a self, entity: E) -> EntityFuture<'a, E, E::Id> {
        Box::pin(async move {
            let id = entity.id();
            let revision = entity.revision();
            let mut entities = self.write()?;
            if revision != 1 {
                return Err(EntityRepositoryError::InvalidInitialRevision {
                    id,
                    actual: revision,
                });
            }
            if entities.contains_key(&id) {
                return Err(EntityRepositoryError::AlreadyExists(id));
            }
            entities.insert(id, entity.clone());
            Ok(entity)
        })
    }

    fn get<'a>(&'a self, id: E::Id) -> EntityFuture<'a, Option<E>, E::Id> {
        Box::pin(async move { Ok(self.read()?.get(&id).cloned()) })
    }

    fn list<'a>(&'a self) -> EntityFuture<'a, Vec<E>, E::Id> {
        Box::pin(async move { Ok(self.read()?.values().cloned().collect()) })
    }

    fn replace<'a>(&'a self, entity: E, expected_revision: u64) -> EntityFuture<'a, E, E::Id> {
        Box::pin(async move {
            let id = entity.id();
            let supplied_revision = entity.revision();
            let mut entities = self.write()?;
            let current = entities
                .get(&id)
                .ok_or(EntityRepositoryError::NotFound(id))?;
            let current_revision = current.revision();
            if current_revision != expected_revision {
                return Err(EntityRepositoryError::Conflict {
                    id,
                    expected: expected_revision,
                    actual: current_revision,
                });
            }
            if supplied_revision <= current_revision {
                return Err(EntityRepositoryError::RevisionNotAdvanced {
                    id,
                    current: current_revision,
                    supplied: supplied_revision,
                });
            }
            entities.insert(id, entity.clone());
            Ok(entity)
        })
    }

    fn delete<'a>(&'a self, id: E::Id, expected_revision: u64) -> EntityFuture<'a, E, E::Id> {
        Box::pin(async move {
            let mut entities = self.write()?;
            let current = entities
                .get(&id)
                .ok_or(EntityRepositoryError::NotFound(id))?;
            let actual_revision = current.revision();
            if actual_revision != expected_revision {
                return Err(EntityRepositoryError::Conflict {
                    id,
                    expected: expected_revision,
                    actual: actual_revision,
                });
            }
            entities
                .remove(&id)
                .ok_or(EntityRepositoryError::NotFound(id))
        })
    }
}
