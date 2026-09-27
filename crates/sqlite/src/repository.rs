//! SQLite-backed declarative resource repositories.
//!
//! One database file holds every resource kind, discriminated by a `kind`
//! column. Each [`SqliteResourceRepository`] instance serves exactly one kind
//! and implements [`ResourceRepository`] with the same error semantics as
//! [`InMemoryResourceRepository`](agentkube_storage::InMemoryResourceRepository).
//!
//! Wire documents are stored as JSON and revalidated through `from_document`
//! on every read, so persisted state always satisfies domain invariants.

use crate::SqliteError;
use agentkube_agents::{AgentDefinition, AgentDeployment};
use agentkube_core::{Namespace, Resource, ResourceVersion};
use agentkube_storage::{ResourceKey, ResourceRepository, StorageError, StorageFuture};
use agentkube_tasks::AgentTask;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS resources (
  kind TEXT NOT NULL,
  namespace TEXT NOT NULL,
  name TEXT NOT NULL,
  uid TEXT NOT NULL,
  resource_version INTEGER NOT NULL CHECK (resource_version > 0),
  document TEXT NOT NULL,
  PRIMARY KEY (kind, namespace, name)
);
CREATE UNIQUE INDEX IF NOT EXISTS resources_uid ON resources (kind, uid);
";

/// One SQLite file opened for every AgentKube resource kind.
pub struct SqliteStores {
    agents: Arc<SqliteResourceRepository>,
    deployments: Arc<SqliteResourceRepository>,
    tasks: Arc<SqliteResourceRepository>,
}

impl SqliteStores {
    /// Opens (creating parent directories) a SQLite store shared by all kinds.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| SqliteError::open(path, error.to_string()))?;
        }
        let connection =
            Connection::open(path).map_err(|error| SqliteError::open(path, error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| SqliteError::schema(error.to_string()))?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .map_err(|error| SqliteError::schema(error.to_string()))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|error| SqliteError::schema(error.to_string()))?;
        let shared = Arc::new(Mutex::new(connection));
        Ok(Self {
            agents: Arc::new(SqliteResourceRepository::for_kind(
                Arc::clone(&shared),
                AgentDefinition::KIND,
            )),
            deployments: Arc::new(SqliteResourceRepository::for_kind(
                Arc::clone(&shared),
                AgentDeployment::KIND,
            )),
            tasks: Arc::new(SqliteResourceRepository::for_kind(
                Arc::clone(&shared),
                AgentTask::KIND,
            )),
        })
    }

    /// Returns the agent repository handle.
    #[must_use]
    pub fn agents(&self) -> Arc<dyn ResourceRepository<AgentDefinition>> {
        Arc::clone(&self.agents) as Arc<dyn ResourceRepository<AgentDefinition>>
    }

    /// Returns the deployment repository handle.
    #[must_use]
    pub fn deployments(&self) -> Arc<dyn ResourceRepository<AgentDeployment>> {
        Arc::clone(&self.deployments) as Arc<dyn ResourceRepository<AgentDeployment>>
    }

    /// Returns the task repository handle.
    #[must_use]
    pub fn tasks(&self) -> Arc<dyn ResourceRepository<AgentTask>> {
        Arc::clone(&self.tasks) as Arc<dyn ResourceRepository<AgentTask>>
    }
}

/// SQLite implementation of [`ResourceRepository`] for one resource kind.
///
/// Statements touch single indexed rows under a short-lived mutex guard that
/// is never held across `.await`.
#[derive(Clone)]
pub struct SqliteResourceRepository {
    connection: Arc<Mutex<Connection>>,
    kind: &'static str,
}

impl SqliteResourceRepository {
    fn for_kind(connection: Arc<Mutex<Connection>>, kind: &'static str) -> Self {
        Self { connection, kind }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::Unavailable("sqlite store lock is poisoned"))
    }

    fn find_row(
        connection: &Connection,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<Option<StoredRow>, StorageError> {
        connection
            .query_row(
                "SELECT document FROM resources
                 WHERE kind = ?1 AND namespace = ?2 AND name = ?3",
                params![kind, namespace, name],
                |row| {
                    Ok(StoredRow {
                        document: row.get(0)?,
                    })
                },
            )
            .optional()
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))
    }

    fn uid_exists(connection: &Connection, kind: &str, uid: &str) -> Result<bool, StorageError> {
        connection
            .query_row(
                "SELECT 1 FROM resources WHERE kind = ?1 AND uid = ?2",
                params![kind, uid],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))
    }

    fn decode<R, Document>(
        encoded: &str,
        from_document: fn(Document) -> Result<R, String>,
    ) -> Result<R, StorageError>
    where
        Document: DeserializeOwned,
    {
        let document: Document = serde_json::from_str(encoded)
            .map_err(|_| StorageError::Unavailable("persisted document failed validation"))?;
        from_document(document)
            .map_err(|_| StorageError::Unavailable("persisted document failed validation"))
    }

    fn create_one<R, Document>(
        &self,
        resource: R,
        into_document: fn(R) -> Document,
        from_document: fn(Document) -> Result<R, String>,
    ) -> Result<R, StorageError>
    where
        R: Resource + Clone,
        Document: Serialize + DeserializeOwned,
    {
        let key = ResourceKey::from(resource.metadata());
        let uid = resource.metadata().uid();
        let version = resource.metadata().resource_version();
        if version != ResourceVersion::INITIAL {
            return Err(StorageError::InvalidInitialVersion {
                key,
                actual: version,
            });
        }
        let namespace = key.namespace().as_str().to_owned();
        let name = key.name().as_str().to_owned();
        let connection = self.lock()?;
        if Self::find_row(&connection, self.kind, &namespace, &name)?.is_some() {
            return Err(StorageError::AlreadyExists(key));
        }
        if Self::uid_exists(&connection, self.kind, &uid.to_string())? {
            return Err(StorageError::UidAlreadyExists(uid));
        }
        let encoded = serde_json::to_string(&into_document(resource.clone()))
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        connection
            .execute(
                "INSERT INTO resources (kind, namespace, name, uid, resource_version, document)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    self.kind,
                    namespace,
                    name,
                    uid.to_string(),
                    version.get() as i64,
                    encoded
                ],
            )
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        // Return through a decode round-trip so the caller observes exactly
        // what durable storage holds.
        Self::decode(&encoded, from_document)
    }

    fn get_one<R, Document>(
        &self,
        key: &ResourceKey,
        from_document: fn(Document) -> Result<R, String>,
    ) -> Result<Option<R>, StorageError>
    where
        Document: DeserializeOwned,
    {
        let connection = self.lock()?;
        let row = Self::find_row(
            &connection,
            self.kind,
            key.namespace().as_str(),
            key.name().as_str(),
        )?;
        row.map(|stored| Self::decode(&stored.document, from_document))
            .transpose()
    }

    fn replace_one<R, Document>(
        &self,
        mut resource: R,
        into_document: fn(R) -> Document,
        from_document: fn(Document) -> Result<R, String>,
    ) -> Result<R, StorageError>
    where
        R: Resource + Clone,
        Document: Serialize + DeserializeOwned,
    {
        let key = ResourceKey::from(resource.metadata());
        let supplied_uid = resource.metadata().uid();
        let supplied_version = resource.metadata().resource_version();
        let namespace = key.namespace().as_str().to_owned();
        let name = key.name().as_str().to_owned();
        let connection = self.lock()?;
        let stored = Self::find_row(&connection, self.kind, &namespace, &name)?
            .ok_or_else(|| StorageError::NotFound(key.clone()))?;
        let current = Self::decode(&stored.document, from_document)?;
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
        let next = current_version
            .next()
            .map_err(|_| StorageError::VersionExhausted(key.clone()))?;
        resource.metadata_mut().set_resource_version(next);
        let encoded = serde_json::to_string(&into_document(resource.clone()))
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        connection
            .execute(
                "UPDATE resources SET resource_version = ?1, document = ?2
                 WHERE kind = ?3 AND namespace = ?4 AND name = ?5",
                params![next.get() as i64, encoded, self.kind, namespace, name],
            )
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        Ok(resource)
    }

    fn delete_one<R, Document>(
        &self,
        key: &ResourceKey,
        expected_version: ResourceVersion,
        from_document: fn(Document) -> Result<R, String>,
    ) -> Result<R, StorageError>
    where
        R: Resource,
        Document: DeserializeOwned,
    {
        let namespace = key.namespace().as_str().to_owned();
        let name = key.name().as_str().to_owned();
        let connection = self.lock()?;
        let stored = Self::find_row(&connection, self.kind, &namespace, &name)?
            .ok_or_else(|| StorageError::NotFound(key.clone()))?;
        let current = Self::decode(&stored.document, from_document)?;
        let actual_version = current.metadata().resource_version();
        if actual_version != expected_version {
            return Err(StorageError::Conflict {
                key: key.clone(),
                expected: expected_version,
                actual: actual_version,
            });
        }
        connection
            .execute(
                "DELETE FROM resources WHERE kind = ?1 AND namespace = ?2 AND name = ?3",
                params![self.kind, namespace, name],
            )
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        Ok(current)
    }

    fn list_decoded<R, Document>(
        &self,
        namespace: Option<Namespace>,
        from_document: fn(Document) -> Result<R, String>,
    ) -> Result<Vec<R>, StorageError>
    where
        Document: DeserializeOwned,
    {
        let filter = namespace.map(|namespace| namespace.as_str().to_owned());
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT document FROM resources
                 WHERE kind = ?1 AND (?2 IS NULL OR namespace = ?2)
                 ORDER BY namespace, name",
            )
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        let rows = statement
            .query_map(params![self.kind, filter], |row| row.get::<_, String>(0))
            .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
        let mut resources = Vec::new();
        for encoded in rows {
            let encoded =
                encoded.map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
            resources.push(Self::decode(&encoded, from_document)?);
        }
        Ok(resources)
    }
}

struct StoredRow {
    document: String,
}

fn agent_from_document(
    document: agentkube_agents::AgentDocument,
) -> Result<AgentDefinition, String> {
    AgentDefinition::from_document(document).map_err(|error| error.to_string())
}

fn deployment_from_document(
    document: agentkube_agents::AgentDeploymentDocument,
) -> Result<AgentDeployment, String> {
    AgentDeployment::from_document(document).map_err(|error| error.to_string())
}

fn task_from_document(document: agentkube_tasks::TaskDocument) -> Result<AgentTask, String> {
    AgentTask::from_document(document).map_err(|error| error.to_string())
}

impl ResourceRepository<AgentDefinition> for SqliteResourceRepository {
    fn create<'a>(&'a self, resource: AgentDefinition) -> StorageFuture<'a, AgentDefinition> {
        Box::pin(async move {
            self.create_one(
                resource,
                AgentDefinition::into_document,
                agent_from_document,
            )
        })
    }

    fn get<'a>(&'a self, key: &'a ResourceKey) -> StorageFuture<'a, Option<AgentDefinition>> {
        Box::pin(async move { self.get_one(key, agent_from_document) })
    }

    fn get_by_uid<'a>(
        &'a self,
        uid: agentkube_core::ResourceUid,
    ) -> StorageFuture<'a, Option<AgentDefinition>> {
        Box::pin(async move {
            let connection = self.lock()?;
            let encoded: Option<String> = connection
                .query_row(
                    "SELECT document FROM resources WHERE kind = ?1 AND uid = ?2",
                    params![self.kind, uid.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
            encoded
                .map(|document| Self::decode(&document, agent_from_document))
                .transpose()
        })
    }

    fn list<'a>(&'a self, namespace: Option<Namespace>) -> StorageFuture<'a, Vec<AgentDefinition>> {
        Box::pin(async move { self.list_decoded(namespace, agent_from_document) })
    }

    fn replace<'a>(&'a self, resource: AgentDefinition) -> StorageFuture<'a, AgentDefinition> {
        Box::pin(async move {
            self.replace_one(
                resource,
                AgentDefinition::into_document,
                agent_from_document,
            )
        })
    }

    fn delete<'a>(
        &'a self,
        key: &'a ResourceKey,
        expected_version: ResourceVersion,
    ) -> StorageFuture<'a, AgentDefinition> {
        Box::pin(async move { self.delete_one(key, expected_version, agent_from_document) })
    }
}

impl ResourceRepository<AgentDeployment> for SqliteResourceRepository {
    fn create<'a>(&'a self, resource: AgentDeployment) -> StorageFuture<'a, AgentDeployment> {
        Box::pin(async move {
            self.create_one(
                resource,
                AgentDeployment::into_document,
                deployment_from_document,
            )
        })
    }

    fn get<'a>(&'a self, key: &'a ResourceKey) -> StorageFuture<'a, Option<AgentDeployment>> {
        Box::pin(async move { self.get_one(key, deployment_from_document) })
    }

    fn get_by_uid<'a>(
        &'a self,
        uid: agentkube_core::ResourceUid,
    ) -> StorageFuture<'a, Option<AgentDeployment>> {
        Box::pin(async move {
            let connection = self.lock()?;
            let encoded: Option<String> = connection
                .query_row(
                    "SELECT document FROM resources WHERE kind = ?1 AND uid = ?2",
                    params![self.kind, uid.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
            encoded
                .map(|document| Self::decode(&document, deployment_from_document))
                .transpose()
        })
    }

    fn list<'a>(&'a self, namespace: Option<Namespace>) -> StorageFuture<'a, Vec<AgentDeployment>> {
        Box::pin(async move { self.list_decoded(namespace, deployment_from_document) })
    }

    fn replace<'a>(&'a self, resource: AgentDeployment) -> StorageFuture<'a, AgentDeployment> {
        Box::pin(async move {
            self.replace_one(
                resource,
                AgentDeployment::into_document,
                deployment_from_document,
            )
        })
    }

    fn delete<'a>(
        &'a self,
        key: &'a ResourceKey,
        expected_version: ResourceVersion,
    ) -> StorageFuture<'a, AgentDeployment> {
        Box::pin(async move { self.delete_one(key, expected_version, deployment_from_document) })
    }
}

impl ResourceRepository<AgentTask> for SqliteResourceRepository {
    fn create<'a>(&'a self, resource: AgentTask) -> StorageFuture<'a, AgentTask> {
        Box::pin(
            async move { self.create_one(resource, AgentTask::into_document, task_from_document) },
        )
    }

    fn get<'a>(&'a self, key: &'a ResourceKey) -> StorageFuture<'a, Option<AgentTask>> {
        Box::pin(async move { self.get_one(key, task_from_document) })
    }

    fn get_by_uid<'a>(
        &'a self,
        uid: agentkube_core::ResourceUid,
    ) -> StorageFuture<'a, Option<AgentTask>> {
        Box::pin(async move {
            let connection = self.lock()?;
            let encoded: Option<String> = connection
                .query_row(
                    "SELECT document FROM resources WHERE kind = ?1 AND uid = ?2",
                    params![self.kind, uid.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| StorageError::Unavailable("sqlite operation failed"))?;
            encoded
                .map(|document| Self::decode(&document, task_from_document))
                .transpose()
        })
    }

    fn list<'a>(&'a self, namespace: Option<Namespace>) -> StorageFuture<'a, Vec<AgentTask>> {
        Box::pin(async move { self.list_decoded(namespace, task_from_document) })
    }

    fn replace<'a>(&'a self, resource: AgentTask) -> StorageFuture<'a, AgentTask> {
        Box::pin(
            async move { self.replace_one(resource, AgentTask::into_document, task_from_document) },
        )
    }

    fn delete<'a>(
        &'a self,
        key: &'a ResourceKey,
        expected_version: ResourceVersion,
    ) -> StorageFuture<'a, AgentTask> {
        Box::pin(async move { self.delete_one(key, expected_version, task_from_document) })
    }
}
