//! Durability and concurrency contract tests for SQLite repositories.
//!
//! The key behavior beyond the in-memory backend: state survives closing and
//! reopening the database file with versions and identities intact.

use agentkube_agents::{
    AgentDefinition, AgentDeployment, AgentDeploymentSpec, AgentRole, AgentSpec, Instructions,
    ModelName, ModelPolicy, ProviderName, ReplicaCount,
};
use agentkube_core::{Metadata, Namespace, Resource};
use agentkube_sqlite::SqliteStores;
use agentkube_storage::{ResourceKey, StorageError};
use agentkube_tasks::{AgentTask, Objective, TaskSpec};

fn agent(name: &str) -> AgentDefinition {
    AgentDefinition::new(
        Metadata::new(name).unwrap(),
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("openai").unwrap(),
                ModelName::new("gpt-5").unwrap(),
            ),
            Instructions::new("Build reliable software.").unwrap(),
        ),
    )
}

fn deployment(name: &str, replicas: u32) -> AgentDeployment {
    AgentDeployment::new(
        Metadata::new(name).unwrap(),
        AgentDeploymentSpec::new(
            ReplicaCount::new(replicas).unwrap(),
            AgentSpec::new(
                AgentRole::new("developer").unwrap(),
                ModelPolicy::fixed(
                    ProviderName::new("openai").unwrap(),
                    ModelName::new("gpt-5").unwrap(),
                ),
                Instructions::new("Serve work.").unwrap(),
            ),
        ),
    )
    .unwrap()
}

fn task(name: &str) -> AgentTask {
    AgentTask::new(
        Metadata::new(name).unwrap(),
        TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap()),
    )
}

fn key(name: &str) -> ResourceKey {
    ResourceKey::new(Namespace::default(), name.parse().unwrap())
}

#[tokio::test]
async fn state_survives_close_and_reopen_with_versions_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agentkube.db");
    let uid;
    {
        let stores = SqliteStores::open(&path).unwrap();
        let created = stores.agents().create(agent("backend")).await.unwrap();
        uid = created.metadata().uid();
        assert_eq!(created.metadata().resource_version().get(), 1);
        stores
            .deployments()
            .create(deployment("workers", 2))
            .await
            .unwrap();
        stores.tasks().create(task("review")).await.unwrap();
        // Bump the agent so reopening must preserve a non-initial version.
        let replaced = stores.agents().replace(created).await.unwrap();
        assert_eq!(replaced.metadata().resource_version().get(), 2);
    }
    // Reopen the same file: everything must still be there.
    {
        let stores = SqliteStores::open(&path).unwrap();
        let fetched = stores.agents().get(&key("backend")).await.unwrap().unwrap();
        assert_eq!(fetched.metadata().uid(), uid);
        assert_eq!(fetched.metadata().resource_version().get(), 2);
        let deployments = stores.deployments().list(None).await.unwrap();
        assert_eq!(deployments.len(), 1);
        assert_eq!(deployments[0].spec().replicas().get(), 2);
        let persisted_task = stores.tasks().get(&key("review")).await.unwrap().unwrap();
        assert_eq!(persisted_task.spec().objective().as_str(), "Execute review");
    }
}

#[tokio::test]
async fn optimistic_concurrency_matches_in_memory_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let stores = SqliteStores::open(dir.path().join("agentkube.db")).unwrap();

    let created = stores.agents().create(agent("planner")).await.unwrap();
    // Duplicate create is a conflict, not an overwrite.
    match stores.agents().create(agent("planner")).await {
        Err(StorageError::AlreadyExists(_)) => {}
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    // Stale replace is a conflict carrying both versions.
    let stale = created.clone();
    let fresh = stores.agents().replace(created).await.unwrap();
    assert_eq!(fresh.metadata().resource_version().get(), 2);
    match stores.agents().replace(stale).await {
        Err(StorageError::Conflict {
            expected, actual, ..
        }) => {
            assert_eq!(expected.get(), 1);
            assert_eq!(actual.get(), 2);
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    // UID changes are rejected even with a current version.
    let mut foreign = agent("planner");
    foreign
        .metadata_mut()
        .set_resource_version(fresh.metadata().resource_version());
    match stores.agents().replace(foreign).await {
        Err(StorageError::IdentityChanged { .. }) => {}
        other => panic!("expected IdentityChanged, got {other:?}"),
    }
    // Delete requires the current version, then the key is gone.
    let deleted = stores
        .agents()
        .delete(&key("planner"), fresh.metadata().resource_version())
        .await
        .unwrap();
    assert_eq!(deleted.metadata().uid(), fresh.metadata().uid());
    assert!(
        stores
            .agents()
            .get(&key("planner"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn lists_are_deterministic_and_namespace_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let stores = SqliteStores::open(dir.path().join("agentkube.db")).unwrap();

    for name in ["charlie", "alpha", "bravo"] {
        stores.agents().create(agent(name)).await.unwrap();
    }
    let listed = stores.agents().list(None).await.unwrap();
    let names: Vec<String> = listed
        .iter()
        .map(|agent| agent.metadata().name().as_str().to_owned())
        .collect();
    assert_eq!(names, vec!["alpha", "bravo", "charlie"]);

    let scoped = stores
        .agents()
        .list(Some(Namespace::new("other").unwrap()))
        .await
        .unwrap();
    assert!(scoped.is_empty());
}

#[tokio::test]
async fn corrupt_documents_surface_as_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agentkube.db");
    let stores = SqliteStores::open(&path).unwrap();
    stores.agents().create(agent("broken")).await.unwrap();
    drop(stores);

    // Corrupt the stored JSON directly through SQL.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE resources SET document = '{not json' WHERE name = 'broken'",
            [],
        )
        .unwrap();
    drop(connection);

    let stores = SqliteStores::open(&path).unwrap();
    match stores.agents().get(&key("broken")).await {
        Err(StorageError::Unavailable(_)) => {}
        other => panic!("expected Unavailable, got {other:?}"),
    }
}
