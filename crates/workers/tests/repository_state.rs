//! Repository-backed worker state: claims persist through the task repo.

use agentkube_agents::{
    AgentDefinition, AgentRole, AgentSpec, Instructions, ModelName, ModelPolicy, ProviderName,
};
use agentkube_core::{Metadata, NodeId};
use agentkube_storage::{InMemoryResourceRepository, ResourceRepository};
use agentkube_tasks::{AgentTask, Objective, TaskSpec, TaskState, TaskUsage};
use agentkube_workers::{RepositoryWorkerStateStore, WorkerStateError, WorkerStateStore};
use std::sync::Arc;

fn make_definition(name: &str) -> AgentDefinition {
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

fn queued_task(name: &str) -> AgentTask {
    let mut task = AgentTask::new(
        Metadata::new(name).unwrap(),
        TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap()),
    );
    task.enqueue().unwrap();
    task
}

fn store_with(tasks: Arc<InMemoryResourceRepository<AgentTask>>) -> RepositoryWorkerStateStore {
    RepositoryWorkerStateStore::new(tasks)
}

#[tokio::test]
async fn claim_and_complete_persist_terminal_state() {
    let repo = Arc::new(InMemoryResourceRepository::new());
    let store = store_with(repo.clone());
    let definition = make_definition("backend");
    store.sync_definitions(vec![definition.clone()]).unwrap();
    let node = NodeId::new();
    let agent_id = store.ensure_instance(&definition, node).unwrap();

    let task = queued_task("build");
    let task_id = task.status().task_id();
    repo.create(task).await.unwrap();

    let claim = store.claim(task_id, agent_id, node).await.unwrap();
    assert_eq!(claim.task().status().state(), TaskState::Running);

    store
        .complete(
            task_id,
            agent_id,
            agentkube_tasks::TaskResult::new("done", TaskUsage::default()),
        )
        .await
        .unwrap();

    let persisted = repo
        .list(None)
        .await
        .unwrap()
        .into_iter()
        .find(|task| task.status().task_id() == task_id)
        .unwrap();
    assert_eq!(persisted.status().state(), TaskState::Completed);
    assert_eq!(persisted.status().result().unwrap().output(), "done");
}

#[tokio::test]
async fn stale_deliveries_are_discarded_without_execution() {
    let repo = Arc::new(InMemoryResourceRepository::new());
    let store = store_with(repo.clone());
    let definition = make_definition("backend");
    store.sync_definitions(vec![definition.clone()]).unwrap();
    let node = NodeId::new();
    let agent_id = store.ensure_instance(&definition, node).unwrap();

    // Unknown task IDs discard immediately.
    match store
        .claim(agentkube_core::TaskId::new(), agent_id, node)
        .await
    {
        Err(WorkerStateError::TaskNotFound(_)) => {}
        other => panic!("expected TaskNotFound, got {other:?}"),
    }

    // A completed task is no longer claimable.
    let task = queued_task("once");
    let task_id = task.status().task_id();
    repo.create(task).await.unwrap();
    store.claim(task_id, agent_id, node).await.unwrap();
    store
        .complete(
            task_id,
            agent_id,
            agentkube_tasks::TaskResult::new("done", TaskUsage::default()),
        )
        .await
        .unwrap();
    match store.claim(task_id, agent_id, node).await {
        Err(WorkerStateError::TaskNotQueued { actual, .. }) => {
            assert_eq!(actual, TaskState::Completed)
        }
        other => panic!("expected TaskNotQueued, got {other:?}"),
    }
}

#[tokio::test]
async fn instances_are_reused_per_definition_and_node() {
    let repo = Arc::new(InMemoryResourceRepository::new());
    let store = store_with(repo);
    let definition = make_definition("backend");
    let node = NodeId::new();

    let first = store.ensure_instance(&definition, node).unwrap();
    let second = store.ensure_instance(&definition, node).unwrap();
    assert_eq!(first, second, "idle ready instances must be reused");

    let other = make_definition("frontend");
    let third = store.ensure_instance(&other, node).unwrap();
    assert_ne!(first, third, "definitions need distinct instances");
}

#[tokio::test]
async fn resync_discovers_tasks_created_behind_the_store() {
    let repo = Arc::new(InMemoryResourceRepository::new());
    let store = store_with(repo.clone());
    let definition = make_definition("backend");
    store.sync_definitions(vec![definition.clone()]).unwrap();
    let node = NodeId::new();
    let agent_id = store.ensure_instance(&definition, node).unwrap();

    // Simulate an API POST that bypasses the worker store entirely.
    let task = queued_task("api-created");
    let task_id = task.status().task_id();
    repo.create(task).await.unwrap();

    // The claim path rescans durable storage on an index miss.
    let claim = store.claim(task_id, agent_id, node).await.unwrap();
    assert_eq!(claim.task().status().assigned_agent(), Some(agent_id));
}
