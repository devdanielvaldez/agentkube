use crate::{ResourceKey, StorageError, StorageFuture, StorageResult};
use agentkube_core::{Namespace, Resource, ResourceUid, ResourceVersion};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

/// Asynchronous persistence port for one declarative resource kind.
pub trait ResourceRepository<R>: Send + Sync
where
    R: Resource + Clone + Send + Sync + 'static,
{
    /// Creates a resource at version one.
    fn create<'a>(&'a self, resource: R) -> StorageFuture<'a, R>;

    /// Retrieves a resource by its namespaced key.
    fn get<'a>(&'a self, key: &'a ResourceKey) -> StorageFuture<'a, Option<R>>;

    /// Retrieves a resource by its immutable UID.
    fn get_by_uid<'a>(&'a self, uid: ResourceUid) -> StorageFuture<'a, Option<R>>;

    /// Lists resources in deterministic key order, optionally within a namespace.
    fn list<'a>(&'a self, namespace: Option<Namespace>) -> StorageFuture<'a, Vec<R>>;

    /// Replaces a resource if its supplied version is current.
    ///
    /// A successful replacement advances the stored resource version exactly
    /// once and returns the persisted value.
    fn replace<'a>(&'a self, resource: R) -> StorageFuture<'a, R>;

    /// Deletes a resource if the expected version is current.
    fn delete<'a>(
        &'a self,
        key: &'a ResourceKey,
        expected_version: ResourceVersion,
    ) -> StorageFuture<'a, R>;
}

/// Thread-safe in-memory implementation of [`ResourceRepository`].
///
/// This backend is intended for tests, local development, and single-process
/// operation. Its key and UID indexes are mutated under one lock so every
/// operation is atomic.
pub struct InMemoryResourceRepository<R> {
    inner: Arc<RwLock<ResourceStore<R>>>,
}

struct ResourceStore<R> {
    by_key: BTreeMap<ResourceKey, R>,
    by_uid: HashMap<ResourceUid, ResourceKey>,
}

impl<R> Default for ResourceStore<R> {
    fn default() -> Self {
        Self {
            by_key: BTreeMap::new(),
            by_uid: HashMap::new(),
        }
    }
}

impl<R> Clone for InMemoryResourceRepository<R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<R> Default for InMemoryResourceRepository<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> InMemoryResourceRepository<R> {
    /// Creates an empty repository.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceStore::default())),
        }
    }

    fn read(&self) -> StorageResult<RwLockReadGuard<'_, ResourceStore<R>>> {
        self.inner
            .read()
            .map_err(|_| StorageError::Unavailable("resource store lock is poisoned"))
    }

    fn write(&self) -> StorageResult<RwLockWriteGuard<'_, ResourceStore<R>>> {
        self.inner
            .write()
            .map_err(|_| StorageError::Unavailable("resource store lock is poisoned"))
    }
}

impl<R> ResourceRepository<R> for InMemoryResourceRepository<R>
where
    R: Resource + Clone + Send + Sync + 'static,
{
    fn create<'a>(&'a self, resource: R) -> StorageFuture<'a, R> {
        Box::pin(async move {
            let key = ResourceKey::from(resource.metadata());
            let uid = resource.metadata().uid();
            let version = resource.metadata().resource_version();
            let mut store = self.write()?;

            if version != ResourceVersion::INITIAL {
                return Err(StorageError::InvalidInitialVersion {
                    key,
                    actual: version,
                });
            }
            if store.by_key.contains_key(&key) {
                return Err(StorageError::AlreadyExists(key));
            }
            if store.by_uid.contains_key(&uid) {
                return Err(StorageError::UidAlreadyExists(uid));
            }

            store.by_uid.insert(uid, key.clone());
            store.by_key.insert(key, resource.clone());
            Ok(resource)
        })
    }

    fn get<'a>(&'a self, key: &'a ResourceKey) -> StorageFuture<'a, Option<R>> {
        Box::pin(async move { Ok(self.read()?.by_key.get(key).cloned()) })
    }

    fn get_by_uid<'a>(&'a self, uid: ResourceUid) -> StorageFuture<'a, Option<R>> {
        Box::pin(async move {
            let store = self.read()?;
            Ok(store
                .by_uid
                .get(&uid)
                .and_then(|key| store.by_key.get(key))
                .cloned())
        })
    }

    fn list<'a>(&'a self, namespace: Option<Namespace>) -> StorageFuture<'a, Vec<R>> {
        Box::pin(async move {
            let store = self.read()?;
            Ok(store
                .by_key
                .iter()
                .filter(|(key, _)| {
                    namespace
                        .as_ref()
                        .is_none_or(|value| key.namespace() == value)
                })
                .map(|(_, resource)| resource.clone())
                .collect())
        })
    }

    fn replace<'a>(&'a self, mut resource: R) -> StorageFuture<'a, R> {
        Box::pin(async move {
            let key = ResourceKey::from(resource.metadata());
            let supplied_uid = resource.metadata().uid();
            let supplied_version = resource.metadata().resource_version();
            let mut store = self.write()?;
            let current = store
                .by_key
                .get(&key)
                .ok_or_else(|| StorageError::NotFound(key.clone()))?;
            let current_uid = current.metadata().uid();
            let current_version = current.metadata().resource_version();

            if current_uid != supplied_uid {
                return Err(StorageError::IdentityChanged {
                    key,
                    expected: current_uid,
                    actual: supplied_uid,
                });
            }
            if current_version != supplied_version {
                return Err(StorageError::Conflict {
                    key,
                    expected: supplied_version,
                    actual: current_version,
                });
            }

            let next_version = current_version
                .next()
                .map_err(|_| StorageError::VersionExhausted(key.clone()))?;
            resource.metadata_mut().set_resource_version(next_version);
            store.by_key.insert(key, resource.clone());
            Ok(resource)
        })
    }

    fn delete<'a>(
        &'a self,
        key: &'a ResourceKey,
        expected_version: ResourceVersion,
    ) -> StorageFuture<'a, R> {
        Box::pin(async move {
            let mut store = self.write()?;
            let current = store
                .by_key
                .get(key)
                .ok_or_else(|| StorageError::NotFound(key.clone()))?;
            let actual_version = current.metadata().resource_version();
            if actual_version != expected_version {
                return Err(StorageError::Conflict {
                    key: key.clone(),
                    expected: expected_version,
                    actual: actual_version,
                });
            }

            let removed = store
                .by_key
                .remove(key)
                .ok_or_else(|| StorageError::NotFound(key.clone()))?;
            store.by_uid.remove(&removed.metadata().uid());
            Ok(removed)
        })
    }
}
