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
    desired_instances: HashMap<ResourceUid, usize>,
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

    /// Removes an instance, returning whether one was present.
    pub fn remove_instance(&self, agent_id: AgentId) -> Result<bool, WorkerStateError> {
        Ok(self.lock()?.agents.remove(&agent_id).is_some())
    }

    /// Restarts a failed instance back to ready through its validated lifecycle.
    pub fn restart_instance(
        &self,
        agent_id: AgentId,
        node_id: NodeId,
    ) -> Result<(), WorkerStateError> {
        let mut inner = self.lock()?;
        let agent = inner
            .agents
            .get_mut(&agent_id)
            .ok_or(WorkerStateError::AgentNotFound(agent_id))?;
        if agent.node_id() != Some(node_id) {
            return Err(WorkerStateError::WrongNode {
                agent_id,
                expected: agent.node_id(),
                actual: node_id,
            });
        }
        if agent.state() != AgentInstanceState::Failed {
            agent.fail().map_err(WorkerStateError::Agent)?;
        }
        agent.retry().map_err(WorkerStateError::Agent)?;
        agent
            .transition(AgentInstanceState::Starting)
            .map_err(WorkerStateError::Agent)?;
        agent
            .transition(AgentInstanceState::Ready)
            .map_err(WorkerStateError::Agent)?;
        Ok(())
    }

    /// Returns a controller-owned replica target, when one was established.
    pub fn desired_instance_count(
        &self,
        definition_uid: ResourceUid,
    ) -> Result<Option<usize>, WorkerStateError> {
        Ok(self.lock()?.desired_instances.get(&definition_uid).copied())
    }

    /// Converges the process-local instances for one definition to `desired`.
    ///
    /// Running instances are never killed during scale-down. Idle excess
    /// replicas are removed first and failed/terminal replicas are replaced.
    pub fn reconcile_instances(
        &self,
        definition: &AgentDefinition,
        node_id: NodeId,
        desired: usize,
    ) -> Result<Vec<AgentInstance>, WorkerStateError> {
        let mut inner = self.lock()?;
        let owner = definition.metadata().uid();
        inner.definitions.insert(owner, definition.clone());
        inner.desired_instances.insert(owner, desired);
        inner.agents.retain(|_, agent| {
            agent.definition_uid() != owner
                || matches!(
                    agent.state(),
                    AgentInstanceState::Ready
                        | AgentInstanceState::Running
                        | AgentInstanceState::Paused
                        | AgentInstanceState::Pending
                        | AgentInstanceState::Scheduling
                        | AgentInstanceState::Starting
                        | AgentInstanceState::Retrying
                )
        });

        let mut idle: Vec<AgentId> = inner
            .agents
            .values()
            .filter(|agent| {
                agent.definition_uid() == owner
                    && agent.current_task().is_none()
                    && agent.state() != AgentInstanceState::Running
            })
            .map(AgentInstance::id)
            .collect();
        idle.sort_by(|left, right| right.as_uuid().cmp(left.as_uuid()));
        let current = inner
            .agents
            .values()
            .filter(|agent| agent.definition_uid() == owner)
            .count();
        for id in idle.into_iter().take(current.saturating_sub(desired)) {
            inner.agents.remove(&id);
        }

        let current = inner
            .agents
            .values()
            .filter(|agent| agent.definition_uid() == owner)
            .count();
        for _ in current..desired {
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
            inner.agents.insert(agent.id(), agent);
        }
        Ok(inner
            .agents
            .values()
            .filter(|agent| agent.definition_uid() == owner)
            .cloned()
            .collect())
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
        let owner = definition.metadata().uid();
        let desired = self
            .lock()?
            .desired_instances
            .get(&owner)
            .copied()
            .unwrap_or(1)
            .max(1);
        self.reconcile_instances(definition, node_id, desired)?
            .into_iter()
            .find(|agent| {
                agent.node_id() == Some(node_id)
                    && agent.state() == AgentInstanceState::Ready
                    && agent.current_task().is_none()
            })
            .map(|agent| agent.id())
            .ok_or(WorkerStateError::Unavailable(
                "definition has no idle instance",
            ))
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
