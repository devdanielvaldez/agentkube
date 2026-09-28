use agentkube_agents::{AgentDefinition, AgentInstance, AgentInstanceError, AgentInstanceState};
use agentkube_core::{AgentId, HumanDuration, NodeId, Resource, ResourceUid, TaskId};
use agentkube_tasks::{AgentTask, TaskFailure, TaskOperationError, TaskResult, TaskState};
use std::{
    collections::HashMap,
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
};

/// Sendable future returned by worker-state operations.
pub type WorkerStateFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, WorkerStateError>> + Send + 'a>>;

/// Atomically claimed task, instance, and immutable agent definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionClaim {
    task: AgentTask,
    agent: AgentInstance,
    definition: AgentDefinition,
}

impl ExecutionClaim {
    /// Creates a claim from its task, agent, and definition snapshots.
    ///
    /// Only [`WorkerStateStore`] implementations construct claims, after
    /// atomically transitioning every part to its running state.
    pub(crate) fn new(task: AgentTask, agent: AgentInstance, definition: AgentDefinition) -> Self {
        Self {
            task,
            agent,
            definition,
        }
    }

    /// Returns the running task snapshot.
    #[must_use]
    pub const fn task(&self) -> &AgentTask {
        &self.task
    }

    /// Returns the running agent-instance snapshot.
    #[must_use]
    pub const fn agent(&self) -> &AgentInstance {
        &self.agent
    }

    /// Returns the immutable agent definition used for execution.
    #[must_use]
    pub const fn definition(&self) -> &AgentDefinition {
        &self.definition
    }
}

/// Queue settlement required after persisting a failed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    /// Retry policy returned the task to `QUEUED`.
    Requeued {
        /// Optional retry backoff.
        delay: Option<HumanDuration>,
    },
    /// Retry policy did not permit another attempt.
    Terminal,
}

/// Atomic task/agent state store used by worker execution.
pub trait WorkerStateStore: Send + Sync {
    /// Transitions a queued task and ready agent to running as one operation.
    fn claim<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        node_id: NodeId,
    ) -> WorkerStateFuture<'a, ExecutionClaim>;

    /// Completes the associated task and releases the agent atomically.
    fn complete<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        result: TaskResult,
    ) -> WorkerStateFuture<'a, ()>;

    /// Fails the task and agent, then applies task retry policy atomically.
    fn fail<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        failure: TaskFailure,
    ) -> WorkerStateFuture<'a, FailureDisposition>;
}

/// Thread-safe transactional worker-state backend for tests and local execution.
#[derive(Clone, Default)]
pub struct InMemoryWorkerStateStore {
    inner: Arc<Mutex<WorkerState>>,
}

#[derive(Default)]
struct WorkerState {
    tasks: HashMap<TaskId, AgentTask>,
    agents: HashMap<AgentId, AgentInstance>,
    definitions: HashMap<ResourceUid, AgentDefinition>,
}

impl InMemoryWorkerStateStore {
    /// Creates an empty worker-state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds a task, rejecting duplicate task IDs.
    pub fn insert_task(&self, task: AgentTask) -> Result<(), WorkerStateError> {
        let id = task.status().task_id();
        let mut state = self.lock()?;
        if state.tasks.contains_key(&id) {
            return Err(WorkerStateError::DuplicateTask(id));
        }
        state.tasks.insert(id, task);
        Ok(())
    }

    /// Seeds an agent instance, rejecting duplicate agent IDs.
    pub fn insert_agent(&self, agent: AgentInstance) -> Result<(), WorkerStateError> {
        let id = agent.id();
        let mut state = self.lock()?;
        if state.agents.contains_key(&id) {
            return Err(WorkerStateError::DuplicateAgent(id));
        }
        state.agents.insert(id, agent);
        Ok(())
    }

    /// Seeds an immutable agent definition, rejecting duplicate UIDs.
    pub fn insert_definition(&self, definition: AgentDefinition) -> Result<(), WorkerStateError> {
        let uid = definition.metadata().uid();
        let mut state = self.lock()?;
        if state.definitions.contains_key(&uid) {
            return Err(WorkerStateError::DuplicateDefinition(uid));
        }
        state.definitions.insert(uid, definition);
        Ok(())
    }

    /// Returns a task snapshot for inspection.
    pub fn task(&self, task_id: TaskId) -> Result<Option<AgentTask>, WorkerStateError> {
        Ok(self.lock()?.tasks.get(&task_id).cloned())
    }

    /// Returns an agent-instance snapshot for inspection.
    pub fn agent(&self, agent_id: AgentId) -> Result<Option<AgentInstance>, WorkerStateError> {
        Ok(self.lock()?.agents.get(&agent_id).cloned())
    }

    fn lock(&self) -> Result<MutexGuard<'_, WorkerState>, WorkerStateError> {
        self.inner
            .lock()
            .map_err(|_| WorkerStateError::Unavailable("worker-state lock is poisoned"))
    }

    fn running_pair(
        state: &WorkerState,
        task_id: TaskId,
        agent_id: AgentId,
    ) -> Result<(AgentTask, AgentInstance), WorkerStateError> {
        let task = state
            .tasks
            .get(&task_id)
            .ok_or(WorkerStateError::TaskNotFound(task_id))?;
        let agent = state
            .agents
            .get(&agent_id)
            .ok_or(WorkerStateError::AgentNotFound(agent_id))?;
        if task.status().state() != TaskState::Running
            || task.status().assigned_agent() != Some(agent_id)
            || agent.state() != AgentInstanceState::Running
            || agent.current_task() != Some(task_id)
        {
            return Err(WorkerStateError::ExecutionMismatch { task_id, agent_id });
        }
        Ok((task.clone(), agent.clone()))
    }
}

impl WorkerStateStore for InMemoryWorkerStateStore {
    fn claim<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        node_id: NodeId,
    ) -> WorkerStateFuture<'a, ExecutionClaim> {
        Box::pin(async move {
            let mut state = self.lock()?;
            let mut task = state
                .tasks
                .get(&task_id)
                .ok_or(WorkerStateError::TaskNotFound(task_id))?
                .clone();
            let mut agent = state
                .agents
                .get(&agent_id)
                .ok_or(WorkerStateError::AgentNotFound(agent_id))?
                .clone();
            if task.status().state() != TaskState::Queued {
                return Err(WorkerStateError::TaskNotQueued {
                    task_id,
                    actual: task.status().state(),
                });
            }
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
            let definition = state
                .definitions
                .get(&agent.definition_uid())
                .ok_or(WorkerStateError::DefinitionNotFound(agent.definition_uid()))?
                .clone();

            task.schedule(agent_id).map_err(WorkerStateError::Task)?;
            task.start().map_err(WorkerStateError::Task)?;
            agent.begin_task(task_id).map_err(WorkerStateError::Agent)?;
            state.tasks.insert(task_id, task.clone());
            state.agents.insert(agent_id, agent.clone());
            Ok(ExecutionClaim::new(task, agent, definition))
        })
    }

    fn complete<'a>(
        &'a self,
        task_id: TaskId,
        agent_id: AgentId,
        result: TaskResult,
    ) -> WorkerStateFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.lock()?;
            let (mut task, mut agent) = Self::running_pair(&state, task_id, agent_id)?;
            task.complete(result).map_err(WorkerStateError::Task)?;
            agent
                .complete_task(task_id)
                .map_err(WorkerStateError::Agent)?;
            state.tasks.insert(task_id, task);
            state.agents.insert(agent_id, agent);
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
            let mut state = self.lock()?;
            let (mut task, mut agent) = Self::running_pair(&state, task_id, agent_id)?;
            task.fail(failure).map_err(WorkerStateError::Task)?;
            agent.fail().map_err(WorkerStateError::Agent)?;
            let delay = task.retry_delay();
            let disposition = match task.retry() {
                Ok(()) => FailureDisposition::Requeued { delay },
                Err(TaskOperationError::RetryNotAllowed) => FailureDisposition::Terminal,
                Err(error) => return Err(WorkerStateError::Task(error)),
            };
            state.tasks.insert(task_id, task);
            state.agents.insert(agent_id, agent);
            Ok(disposition)
        })
    }
}

/// Invalid or unavailable worker execution state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerStateError {
    /// A task ID is already present.
    DuplicateTask(TaskId),
    /// An agent ID is already present.
    DuplicateAgent(AgentId),
    /// An agent-definition UID is already present.
    DuplicateDefinition(ResourceUid),
    /// The queue referenced a missing task.
    TaskNotFound(TaskId),
    /// The requested agent instance does not exist.
    AgentNotFound(AgentId),
    /// The instance references a missing agent definition.
    DefinitionNotFound(ResourceUid),
    /// The task is not available to claim.
    TaskNotQueued {
        /// Requested task.
        task_id: TaskId,
        /// Current lifecycle state.
        actual: TaskState,
    },
    /// The agent cannot begin work.
    AgentNotReady {
        /// Requested agent.
        agent_id: AgentId,
        /// Current lifecycle state.
        actual: AgentInstanceState,
    },
    /// The agent belongs to another worker node.
    WrongNode {
        /// Requested agent.
        agent_id: AgentId,
        /// Assigned node, if any.
        expected: Option<NodeId>,
        /// Worker attempting execution.
        actual: NodeId,
    },
    /// Persisted task and agent do not describe the same running execution.
    ExecutionMismatch {
        /// Task being settled.
        task_id: TaskId,
        /// Agent being settled.
        agent_id: AgentId,
    },
    /// Task lifecycle mutation failed.
    Task(TaskOperationError),
    /// Agent lifecycle mutation failed.
    Agent(AgentInstanceError),
    /// State backend is unavailable.
    Unavailable(&'static str),
}

impl WorkerStateError {
    pub(crate) const fn discards_delivery(&self) -> bool {
        matches!(self, Self::TaskNotFound(_) | Self::TaskNotQueued { .. })
    }
}

impl fmt::Display for WorkerStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTask(id) => write!(formatter, "task {id} already exists"),
            Self::DuplicateAgent(id) => write!(formatter, "agent {id} already exists"),
            Self::DuplicateDefinition(uid) => write!(formatter, "definition {uid} already exists"),
            Self::TaskNotFound(id) => write!(formatter, "task {id} was not found"),
            Self::AgentNotFound(id) => write!(formatter, "agent {id} was not found"),
            Self::DefinitionNotFound(uid) => write!(formatter, "definition {uid} was not found"),
            Self::TaskNotQueued { task_id, actual } => {
                write!(formatter, "task {task_id} is {actual:?}, not QUEUED")
            }
            Self::AgentNotReady { agent_id, actual } => {
                write!(formatter, "agent {agent_id} is {actual:?}, not READY")
            }
            Self::WrongNode {
                agent_id,
                expected,
                actual,
            } => write!(
                formatter,
                "agent {agent_id} is assigned to {expected:?}, not node {actual}"
            ),
            Self::ExecutionMismatch { task_id, agent_id } => {
                write!(
                    formatter,
                    "task {task_id} and agent {agent_id} are not one execution"
                )
            }
            Self::Task(error) => write!(formatter, "task lifecycle failed: {error}"),
            Self::Agent(error) => write!(formatter, "agent lifecycle failed: {error}"),
            Self::Unavailable(reason) => write!(formatter, "worker state unavailable: {reason}"),
        }
    }
}

impl Error for WorkerStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Task(error) => Some(error),
            Self::Agent(error) => Some(error),
            _ => None,
        }
    }
}
