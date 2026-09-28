//! Repository-backed worker state for embedded operators.
//!
//! Unlike [`InMemoryWorkerStateStore`](crate::InMemoryWorkerStateStore), which
//! is seeded manually, this store resolves tasks through a persistent
//! [`ResourceRepository`](agentkube_storage::ResourceRepository), so task
//! state survives restarts. Agent instances and definition snapshots stay
//! process-local: they describe live execution capacity, not durable truth.
//!
//! Optimistic concurrency keeps concurrent claims honest: a lost replace race
//! is reported as [`WorkerStateError::TaskNotQueued`], letting the worker
//! discard its delivery while the winning claim executes.

use crate::{
    ExecutionClaim, FailureDisposition, WorkerStateError, WorkerStateFuture, WorkerStateStore,
};
use agentkube_agents::{AgentDefinition, AgentInstance, AgentInstanceState};
use agentkube_core::{AgentId, NodeId, Resource, ResourceUid, TaskId};
use agentkube_storage::{ResourceKey, ResourceRepository};
use agentkube_tasks::{AgentTask, TaskFailure, TaskOperationError, TaskResult, TaskState};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Default)]
struct StoreInner {
    index: HashMap<TaskId, ResourceKey>,
    agents: HashMap<AgentId, AgentInstance>,
    definitions: HashMap<ResourceUid, AgentDefinition>,
}

impl StoreInner {
    fn load_agent(
        &self,
        agent_id: AgentId,
        node_id: NodeId,
    ) -> Result<AgentInstance, WorkerStateError> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(WorkerStateError::AgentNotFound(agent_id))?;
        if agent.state() != AgentInstanceState::Ready {
            return Err(WorkerStateError::AgentNotReady {
                agent_id,
                actual: agent.state(),
            });
        }
        if agent.node_id() != Some(node_id) {
            return Err(WorkerStateError::WrongNode {
                agent_id,
                expected: agent.node_id(),
                actual: node_id,
            });
        }
        Ok(agent.clone())
    }

    /// Loads the agent side of a live execution, verifying both halves
    /// describe the same running task.
    fn load_running(
        &self,
        task: &AgentTask,
        task_id: TaskId,
        agent_id: AgentId,
    ) -> Result<AgentInstance, WorkerStateError> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(WorkerStateError::AgentNotFound(agent_id))?
            .clone();
        if task.status().state() != TaskState::Running
            || task.status().assigned_agent() != Some(agent_id)
            || agent.state() != AgentInstanceState::Running
            || agent.current_task() != Some(task_id)
        {
            return Err(WorkerStateError::ExecutionMismatch { task_id, agent_id });
        }
        Ok(agent)
    }
}

/// Worker state persisted through a task repository.
///
/// Cloning shares the same underlying maps and repository handle.
#[derive(Clone)]
pub struct RepositoryWorkerStateStore {
    tasks: Arc<dyn ResourceRepository<AgentTask>>,
    inner: Arc<Mutex<StoreInner>>,
}

impl RepositoryWorkerStateStore {
    /// Creates a store over a task repository. Call [`resync`](Self::resync)
    /// before serving traffic so tasks created while offline are indexed.
    #[must_use]
    pub fn new(tasks: Arc<dyn ResourceRepository<AgentTask>>) -> Self {
        Self {
            tasks,
            inner: Arc::new(Mutex::new(StoreInner::default())),
        }
    }

    /// Rebuilds the task-identity index from durable storage.
    pub async fn resync(&self) -> Result<(), WorkerStateError> {
        let tasks = self
            .tasks
            .list(None)
            .await
            .map_err(|_| WorkerStateError::Unavailable("task repository unavailable"))?;
        let mut inner = self.lock()?;
        inner.index.clear();
        for task in tasks {
            inner
                .index
                .insert(task.status().task_id(), ResourceKey::from(task.metadata()));
        }
        Ok(())
    }

    /// Replaces the definition snapshots with the current repository listing.
    pub fn sync_definitions(
        &self,
        definitions: Vec<AgentDefinition>,
    ) -> Result<(), WorkerStateError> {
        let mut inner = self.lock()?;
        inner.definitions.clear();
        for definition in definitions {
            inner
                .definitions
                .insert(definition.metadata().uid(), definition);
        }
        Ok(())
    }

    /// Snapshots live agent instances for scheduling and reconciliation.
    pub fn instances(&self) -> Result<Vec<AgentInstance>, WorkerStateError> {
        Ok(self.lock()?.agents.values().cloned().collect())
    }

    /// Returns a ready, idle instance for a definition, creating one when needed.
    ///
    /// Instances that can never run again (completed, terminated, failed) are
    /// dropped for the definition; failure evidence stays in persisted task
    /// status, which this map does not own.
    pub fn ensure_instance(
        &self,
        definition: &AgentDefinition,
        node_id: NodeId,
    ) -> Result<AgentId, WorkerStateError> {
        let mut inner = self.lock()?;
        let owner = definition.metadata().uid();
        inner.definitions.insert(owner, definition.clone());
        if let Some((id, _)) = inner
            .agents
            .iter()
            .find(|(_, agent)| {
                agent.definition_uid() == owner
                    && agent.node_id() == Some(node_id)
                    && agent.state() == AgentInstanceState::Ready
                    && agent.current_task().is_none()
            })
            .map(|(id, agent)| (*id, agent.clone()))
        {
            return Ok(id);
        }
        inner.agents.retain(|_, agent| {
            agent.definition_uid() != owner
                || agent.state() == AgentInstanceState::Running
                || agent.current_task().is_some()
        });
        let mut agent = AgentInstance::new(owner);
        agent
            .transition(AgentInstanceState::Scheduling)
            .map_err(WorkerStateError::Agent)?;
        agent
            .assign_node(node_id)
            .map_err(WorkerStateError::Agent)?;
        agent
            .transition(AgentInstanceState::Starting)
            .map_err(WorkerStateError::Agent)?;
        agent
            .transition(AgentInstanceState::Ready)
            .map_err(WorkerStateError::Agent)?;
        let id = agent.id();
        inner.agents.insert(id, agent);
        Ok(id)
    }

    fn lock(&self) -> Result<MutexGuard<'_, StoreInner>, WorkerStateError> {
        self.inner
            .lock()
            .map_err(|_| WorkerStateError::Unavailable("worker-state lock is poisoned"))
    }

    async fn resolve(&self, task_id: TaskId) -> Result<(ResourceKey, AgentTask), WorkerStateError> {
        // The mutex guard must never be held across `.await`: extract owned
        // data first so every future stays `Send`.
        let cached: Option<ResourceKey> = self.lock()?.index.get(&task_id).cloned();
        if let Some(key) = cached {
            match self.tasks.get(&key).await {
                Ok(Some(task)) => return Ok((key, task)),
                Ok(None) => {
                    self.lock()?.index.remove(&task_id);
                }
                Err(_) => {
                    return Err(WorkerStateError::Unavailable("task repository unavailable"));
                }
            }
        }
        self.resync().await?;
        let retried: Option<ResourceKey> = self.lock()?.index.get(&task_id).cloned();
        match retried {
            Some(key) => match self.tasks.get(&key).await {
                Ok(Some(task)) => Ok((key, task)),
                Ok(None) | Err(_) => Err(WorkerStateError::TaskNotFound(task_id)),
            },
            None => Err(WorkerStateError::TaskNotFound(task_id)),
        }
    }
}

impl WorkerStateStore for RepositoryWorkerStateStore {
    fn claim<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        node_id: NodeId,
    ) -> WorkerStateFuture<'a, ExecutionClaim> {
        Box::pin(async move {
            let (_, mut task) = self.resolve(task_id).await?;
            if task.status().state() != TaskState::Queued {
                return Err(WorkerStateError::TaskNotQueued {
                    task_id,
                    actual: task.status().state(),
                });
            }
            let mut agent = self.lock()?.load_agent(agent_id, node_id)?;
            let definition = self
                .lock()?
                .definitions
                .get(&agent.definition_uid())
                .cloned()
                .ok_or(WorkerStateError::DefinitionNotFound(agent.definition_uid()))?;
            task.schedule(agent_id).map_err(WorkerStateError::Task)?;
            task.start().map_err(WorkerStateError::Task)?;
            agent.begin_task(task_id).map_err(WorkerStateError::Agent)?;
            // A lost replace race means another claim won: report the fresh
            // state so the worker discards its delivery instead of executing.
            match self.tasks.replace(task).await {
                Ok(_) => {}
                Err(agentkube_storage::StorageError::Conflict { .. })
                | Err(agentkube_storage::StorageError::NotFound(_)) => {
                    let (_, current) = self.resolve(task_id).await?;
                    return Err(WorkerStateError::TaskNotQueued {
                        task_id,
                        actual: current.status().state(),
                    });
                }
                Err(_) => {
                    return Err(WorkerStateError::Unavailable("task repository unavailable"));
                }
            }
            self.lock()?.agents.insert(agent_id, agent.clone());
            // Reload the persisted running snapshot so the claim observes
            // exactly what durable storage holds.
            let (_, persisted) = self.resolve(task_id).await?;
            Ok(ExecutionClaim::new(persisted, agent, definition))
        })
    }

    fn complete<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        result: TaskResult,
    ) -> WorkerStateFuture<'a, ()> {
        Box::pin(async move {
            let (_, mut task) = self.resolve(task_id).await?;
            let mut agent = self.lock()?.load_running(&task, task_id, agent_id)?;
            task.complete(result).map_err(WorkerStateError::Task)?;
            agent
                .complete_task(task_id)
                .map_err(WorkerStateError::Agent)?;
            self.tasks
                .replace(task)
                .await
                .map_err(|_| WorkerStateError::Unavailable("task repository unavailable"))?;
            self.lock()?.agents.insert(agent_id, agent);
            Ok(())
        })
    }

    fn fail<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        failure: TaskFailure,
    ) -> WorkerStateFuture<'a, FailureDisposition> {
        Box::pin(async move {
            let (_, mut task) = self.resolve(task_id).await?;
            let mut agent = self.lock()?.load_running(&task, task_id, agent_id)?;
            task.fail(failure).map_err(WorkerStateError::Task)?;
            agent.fail().map_err(WorkerStateError::Agent)?;
            let delay = task.retry_delay();
            let disposition = match task.retry() {
                Ok(()) => FailureDisposition::Requeued { delay },
                Err(TaskOperationError::RetryNotAllowed) => FailureDisposition::Terminal,
                Err(error) => return Err(WorkerStateError::Task(error)),
            };
            self.tasks
                .replace(task)
                .await
                .map_err(|_| WorkerStateError::Unavailable("task repository unavailable"))?;
            self.lock()?.agents.insert(agent_id, agent);
            Ok(disposition)
        })
    }
}
