use agentkube_core::{AgentId, NodeId, ResourceUid, TaskId};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt};

/// Runtime state of a concrete agent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentInstanceState {
    /// Created but not yet considered by the scheduler.
    Pending,
    /// Waiting for or receiving node assignment.
    Scheduling,
    /// Runtime initialization is in progress.
    Starting,
    /// Available to accept a task.
    Ready,
    /// Executing a task.
    Running,
    /// Execution is intentionally paused.
    Paused,
    /// Work completed and the instance will not accept more tasks.
    Completed,
    /// Execution failed and may be retried.
    Failed,
    /// A replacement execution attempt is being prepared.
    Retrying,
    /// Permanently stopped.
    Terminated,
}

impl AgentInstanceState {
    /// Returns whether no further state transition is allowed.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Terminated)
    }

    const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Scheduling | Self::Terminated)
                | (
                    Self::Scheduling,
                    Self::Starting | Self::Failed | Self::Terminated
                )
                | (
                    Self::Starting,
                    Self::Ready | Self::Failed | Self::Terminated
                )
                | (Self::Ready, Self::Running | Self::Failed | Self::Terminated)
                | (
                    Self::Running,
                    Self::Ready | Self::Paused | Self::Completed | Self::Failed | Self::Terminated
                )
                | (
                    Self::Paused,
                    Self::Running | Self::Failed | Self::Terminated
                )
                | (Self::Failed, Self::Retrying | Self::Terminated)
                | (
                    Self::Retrying,
                    Self::Starting | Self::Failed | Self::Terminated
                )
        )
    }
}

/// Concrete schedulable execution of an [`crate::AgentDefinition`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInstance {
    id: AgentId,
    definition_uid: ResourceUid,
    state: AgentInstanceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node_id: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_task: Option<TaskId>,
    restart_count: u32,
    revision: u64,
}

impl AgentInstance {
    /// Creates a pending instance for an immutable definition UID.
    #[must_use]
    pub fn new(definition_uid: ResourceUid) -> Self {
        Self {
            id: AgentId::new(),
            definition_uid,
            state: AgentInstanceState::Pending,
            node_id: None,
            current_task: None,
            restart_count: 0,
            revision: 1,
        }
    }

    /// Performs a validated lifecycle transition.
    pub fn transition(&mut self, next: AgentInstanceState) -> Result<(), AgentInstanceError> {
        if matches!(
            (self.state, next),
            (AgentInstanceState::Ready, AgentInstanceState::Running)
                | (
                    AgentInstanceState::Running,
                    AgentInstanceState::Ready | AgentInstanceState::Completed
                )
                | (_, AgentInstanceState::Failed)
        ) {
            return Err(AgentInstanceError::CoordinatedTransitionRequired {
                from: self.state,
                to: next,
            });
        }
        if self.state == AgentInstanceState::Scheduling
            && next == AgentInstanceState::Starting
            && self.node_id.is_none()
        {
            return Err(AgentInstanceError::MissingNodeAssignment);
        }
        self.apply_transition(next)?;
        if next == AgentInstanceState::Terminated {
            self.current_task = None;
        }
        Ok(())
    }

    /// Assigns a node while the instance is being scheduled.
    pub fn assign_node(&mut self, node_id: NodeId) -> Result<(), AgentInstanceError> {
        if self.state != AgentInstanceState::Scheduling {
            return Err(AgentInstanceError::OperationRequiresState {
                operation: "assign_node",
                required: AgentInstanceState::Scheduling,
                actual: self.state,
            });
        }
        if self.node_id.is_some() {
            return Err(AgentInstanceError::NodeAlreadyAssigned);
        }
        self.bump_revision()?;
        self.node_id = Some(node_id);
        Ok(())
    }

    /// Starts a task and atomically enters `RUNNING`.
    pub fn begin_task(&mut self, task_id: TaskId) -> Result<(), AgentInstanceError> {
        if self.current_task.is_some() {
            return Err(AgentInstanceError::TaskAlreadyAssigned);
        }
        self.apply_transition(AgentInstanceState::Running)?;
        self.current_task = Some(task_id);
        Ok(())
    }

    /// Completes the current task and atomically returns to `READY`.
    pub fn complete_task(&mut self, task_id: TaskId) -> Result<(), AgentInstanceError> {
        self.require_current_task(task_id)?;
        self.apply_transition(AgentInstanceState::Ready)?;
        self.current_task = None;
        Ok(())
    }

    /// Completes the current task and permanently completes the instance.
    pub fn complete_instance(&mut self, task_id: TaskId) -> Result<(), AgentInstanceError> {
        self.require_current_task(task_id)?;
        self.apply_transition(AgentInstanceState::Completed)?;
        self.current_task = None;
        Ok(())
    }

    /// Marks execution failed and releases any current task assignment.
    pub fn fail(&mut self) -> Result<(), AgentInstanceError> {
        self.apply_transition(AgentInstanceState::Failed)?;
        self.current_task = None;
        Ok(())
    }

    /// Begins a new execution attempt after failure.
    pub fn retry(&mut self) -> Result<(), AgentInstanceError> {
        if self.state != AgentInstanceState::Failed {
            return Err(AgentInstanceError::OperationRequiresState {
                operation: "retry",
                required: AgentInstanceState::Failed,
                actual: self.state,
            });
        }
        let restart_count = self
            .restart_count
            .checked_add(1)
            .ok_or(AgentInstanceError::RestartCountExhausted)?;
        self.apply_transition(AgentInstanceState::Retrying)?;
        self.restart_count = restart_count;
        Ok(())
    }

    /// Returns the instance identifier.
    #[must_use]
    pub const fn id(&self) -> AgentId {
        self.id
    }

    /// Returns the immutable definition UID.
    #[must_use]
    pub const fn definition_uid(&self) -> ResourceUid {
        self.definition_uid
    }

    /// Returns the lifecycle state.
    #[must_use]
    pub const fn state(&self) -> AgentInstanceState {
        self.state
    }

    /// Returns the assigned node.
    #[must_use]
    pub const fn node_id(&self) -> Option<NodeId> {
        self.node_id
    }

    /// Returns the currently executing task.
    #[must_use]
    pub const fn current_task(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Returns the number of retries begun after failure.
    #[must_use]
    pub const fn restart_count(&self) -> u32 {
        self.restart_count
    }

    /// Returns the optimistic-concurrency revision of runtime state.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    fn require_current_task(&self, task_id: TaskId) -> Result<(), AgentInstanceError> {
        match self.current_task {
            Some(current) if current == task_id => Ok(()),
            Some(current) => Err(AgentInstanceError::TaskMismatch {
                expected: current,
                actual: task_id,
            }),
            None => Err(AgentInstanceError::NoCurrentTask),
        }
    }

    fn bump_revision(&mut self) -> Result<(), AgentInstanceError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(AgentInstanceError::RevisionExhausted)?;
        Ok(())
    }

    fn apply_transition(&mut self, next: AgentInstanceState) -> Result<(), AgentInstanceError> {
        if !self.state.can_transition_to(next) {
            return Err(AgentInstanceError::InvalidTransition {
                from: self.state,
                to: next,
            });
        }
        self.bump_revision()?;
        self.state = next;
        Ok(())
    }

    fn validate_snapshot(&self) -> Result<(), AgentInstanceError> {
        if self.revision == 0 {
            return Err(AgentInstanceError::InvalidSnapshot(
                "revision must be greater than zero",
            ));
        }
        if self.state == AgentInstanceState::Pending
            && (self.node_id.is_some() || self.current_task.is_some())
        {
            return Err(AgentInstanceError::InvalidSnapshot(
                "pending instance cannot have a node or task",
            ));
        }
        if matches!(
            self.state,
            AgentInstanceState::Starting
                | AgentInstanceState::Ready
                | AgentInstanceState::Running
                | AgentInstanceState::Paused
                | AgentInstanceState::Completed
        ) && self.node_id.is_none()
        {
            return Err(AgentInstanceError::InvalidSnapshot(
                "active instance must have an assigned node",
            ));
        }
        let task_required = matches!(
            self.state,
            AgentInstanceState::Running | AgentInstanceState::Paused
        );
        if task_required != self.current_task.is_some() {
            return Err(AgentInstanceError::InvalidSnapshot(
                "task assignment does not match lifecycle state",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentInstanceWire {
    id: AgentId,
    definition_uid: ResourceUid,
    state: AgentInstanceState,
    #[serde(default)]
    node_id: Option<NodeId>,
    #[serde(default)]
    current_task: Option<TaskId>,
    restart_count: u32,
    revision: u64,
}

impl<'de> Deserialize<'de> for AgentInstance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AgentInstanceWire::deserialize(deserializer)?;
        let instance = Self {
            id: wire.id,
            definition_uid: wire.definition_uid,
            state: wire.state,
            node_id: wire.node_id,
            current_task: wire.current_task,
            restart_count: wire.restart_count,
            revision: wire.revision,
        };
        instance.validate_snapshot().map_err(de::Error::custom)?;
        Ok(instance)
    }
}

/// Invalid agent-instance lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentInstanceError {
    /// The state-machine edge is not permitted.
    InvalidTransition {
        /// Current state.
        from: AgentInstanceState,
        /// Requested state.
        to: AgentInstanceState,
    },
    /// A transition must use an atomic task or failure operation.
    CoordinatedTransitionRequired {
        /// Current state.
        from: AgentInstanceState,
        /// Requested state.
        to: AgentInstanceState,
    },
    /// An operation was attempted from the wrong lifecycle state.
    OperationRequiresState {
        /// Stable operation name.
        operation: &'static str,
        /// Required lifecycle state.
        required: AgentInstanceState,
        /// Actual lifecycle state.
        actual: AgentInstanceState,
    },
    /// The scheduler attempted to replace an existing node assignment.
    NodeAlreadyAssigned,
    /// Runtime startup was requested before the scheduler assigned a node.
    MissingNodeAssignment,
    /// A task is already executing.
    TaskAlreadyAssigned,
    /// No task is currently assigned.
    NoCurrentTask,
    /// A completion referred to a task other than the assigned one.
    TaskMismatch {
        /// Assigned task.
        expected: TaskId,
        /// Supplied task.
        actual: TaskId,
    },
    /// The retry counter cannot be incremented.
    RestartCountExhausted,
    /// The runtime-state revision cannot be incremented.
    RevisionExhausted,
    /// A persisted or wire snapshot violates lifecycle invariants.
    InvalidSnapshot(&'static str),
}

impl fmt::Display for AgentInstanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { from, to } => {
                write!(
                    formatter,
                    "invalid agent transition from {from:?} to {to:?}"
                )
            }
            Self::CoordinatedTransitionRequired { from, to } => write!(
                formatter,
                "transition from {from:?} to {to:?} requires an atomic lifecycle operation"
            ),
            Self::OperationRequiresState {
                operation,
                required,
                actual,
            } => write!(
                formatter,
                "operation {operation} requires {required:?}, but instance is {actual:?}"
            ),
            Self::NodeAlreadyAssigned => formatter.write_str("agent node is already assigned"),
            Self::MissingNodeAssignment => {
                formatter.write_str("agent cannot start before a node is assigned")
            }
            Self::TaskAlreadyAssigned => formatter.write_str("agent already has a task"),
            Self::NoCurrentTask => formatter.write_str("agent has no current task"),
            Self::TaskMismatch { expected, actual } => {
                write!(
                    formatter,
                    "expected current task {expected}, received {actual}"
                )
            }
            Self::RestartCountExhausted => formatter.write_str("restart counter is exhausted"),
            Self::RevisionExhausted => formatter.write_str("instance revision is exhausted"),
            Self::InvalidSnapshot(message) => {
                write!(formatter, "invalid agent instance snapshot: {message}")
            }
        }
    }
}

impl Error for AgentInstanceError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_instance() -> AgentInstance {
        let mut instance = AgentInstance::new(ResourceUid::new());
        instance.transition(AgentInstanceState::Scheduling).unwrap();
        instance.assign_node(NodeId::new()).unwrap();
        instance.transition(AgentInstanceState::Starting).unwrap();
        instance.transition(AgentInstanceState::Ready).unwrap();
        instance
    }

    #[test]
    fn instance_executes_and_completes_a_task() {
        let mut instance = ready_instance();
        let task = TaskId::new();

        instance.begin_task(task).unwrap();
        assert_eq!(instance.state(), AgentInstanceState::Running);
        assert_eq!(instance.current_task(), Some(task));

        instance.complete_task(task).unwrap();
        assert_eq!(instance.state(), AgentInstanceState::Ready);
        assert_eq!(instance.current_task(), None);
    }

    #[test]
    fn instance_rejects_skipping_lifecycle_stages() {
        let mut instance = AgentInstance::new(ResourceUid::new());
        let error = instance.transition(AgentInstanceState::Ready).unwrap_err();

        assert!(matches!(
            error,
            AgentInstanceError::InvalidTransition { .. }
        ));
        assert_eq!(instance.state(), AgentInstanceState::Pending);
    }

    #[test]
    fn failed_instance_can_begin_a_counted_retry() {
        let mut instance = ready_instance();
        instance.fail().unwrap();
        instance.retry().unwrap();

        assert_eq!(instance.state(), AgentInstanceState::Retrying);
        assert_eq!(instance.restart_count(), 1);
    }

    #[test]
    fn wrong_task_cannot_complete_the_current_execution() {
        let mut instance = ready_instance();
        let current = TaskId::new();
        instance.begin_task(current).unwrap();

        assert!(matches!(
            instance.complete_task(TaskId::new()),
            Err(AgentInstanceError::TaskMismatch { .. })
        ));
        assert_eq!(instance.current_task(), Some(current));
    }

    #[test]
    fn generic_transition_cannot_bypass_task_coordination() {
        let mut instance = ready_instance();

        assert!(matches!(
            instance.transition(AgentInstanceState::Running),
            Err(AgentInstanceError::CoordinatedTransitionRequired { .. })
        ));
        assert_eq!(instance.state(), AgentInstanceState::Ready);
    }

    #[test]
    fn deserialization_rejects_running_instance_without_a_task() {
        let instance = ready_instance();
        let mut value = serde_json::to_value(instance).unwrap();
        value["state"] = serde_json::json!("RUNNING");

        assert!(serde_json::from_value::<AgentInstance>(value).is_err());
    }
}
