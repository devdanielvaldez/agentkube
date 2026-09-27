use agentkube_core::{AgentId, TaskId};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt};

/// Lifecycle state of an agent task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskState {
    /// Created but not submitted to a queue.
    Pending,
    /// Available for scheduling.
    Queued,
    /// Assigned to an agent but not yet executing.
    Scheduled,
    /// Actively executing.
    Running,
    /// Waiting for a tool call to complete.
    WaitingTool,
    /// Waiting for another agent.
    WaitingAgent,
    /// Waiting for human approval.
    WaitingApproval,
    /// Finished successfully.
    Completed,
    /// Execution ended with an error.
    Failed,
    /// Cancelled before successful completion.
    Cancelled,
}

impl TaskState {
    /// Returns whether no more execution work should occur.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Returns whether execution is paused on an external dependency.
    #[must_use]
    pub const fn is_waiting(self) -> bool {
        matches!(
            self,
            Self::WaitingTool | Self::WaitingAgent | Self::WaitingApproval
        )
    }
}

/// Stable failure category used by retry policy and observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskFailureKind {
    /// Model or tool provider rejected traffic due to a rate limit.
    RateLimit,
    /// An operation exceeded its deadline.
    Timeout,
    /// A model provider returned an operational error.
    ProviderError,
    /// The assigned worker disappeared or became unhealthy.
    WorkerLost,
    /// Required permission was denied.
    PermissionDenied,
    /// Task input or output failed validation.
    Validation,
    /// The configured token or financial budget was exhausted.
    BudgetExceeded,
    /// An unexpected execution failure occurred.
    Internal,
}

/// Structured task failure retained for retry decisions and auditing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskFailure {
    kind: TaskFailureKind,
    message: String,
}

impl TaskFailure {
    /// Creates a task failure.
    #[must_use]
    pub fn new(kind: TaskFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(&self) -> TaskFailureKind {
        self.kind
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Metered resources consumed by task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskUsage {
    input_tokens: u64,
    output_tokens: u64,
    cost_micro_usd: u64,
}

impl TaskUsage {
    /// Creates usage counters.
    #[must_use]
    pub const fn new(input_tokens: u64, output_tokens: u64, cost_micro_usd: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cost_micro_usd,
        }
    }

    /// Returns prompt/input tokens.
    #[must_use]
    pub const fn input_tokens(self) -> u64 {
        self.input_tokens
    }

    /// Returns completion/output tokens.
    #[must_use]
    pub const fn output_tokens(self) -> u64 {
        self.output_tokens
    }

    /// Returns total tokens when the sum fits in `u64`.
    #[must_use]
    pub const fn total_tokens(self) -> Option<u64> {
        self.input_tokens.checked_add(self.output_tokens)
    }

    /// Returns cost in millionths of one US dollar.
    #[must_use]
    pub const fn cost_micro_usd(self) -> u64 {
        self.cost_micro_usd
    }
}

/// Successful task output and resource usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskResult {
    output: String,
    #[serde(default)]
    usage: TaskUsage,
}

impl TaskResult {
    /// Creates a successful result. Output may be empty for side-effect-only tasks.
    #[must_use]
    pub fn new(output: impl Into<String>, usage: TaskUsage) -> Self {
        Self {
            output: output.into(),
            usage,
        }
    }

    /// Returns the task output.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }

    /// Returns metered usage.
    #[must_use]
    pub const fn usage(&self) -> TaskUsage {
        self.usage
    }
}

/// Observed task execution state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStatus {
    task_id: TaskId,
    state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assigned_agent: Option<AgentId>,
    attempts_started: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<TaskResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<TaskFailure>,
    revision: u64,
}

impl TaskStatus {
    /// Creates pending status with a globally unique task identifier.
    #[must_use]
    pub fn pending() -> Self {
        Self {
            task_id: TaskId::new(),
            state: TaskState::Pending,
            assigned_agent: None,
            attempts_started: 0,
            result: None,
            failure: None,
            revision: 1,
        }
    }

    /// Returns the task identifier.
    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Returns the lifecycle state.
    #[must_use]
    pub const fn state(&self) -> TaskState {
        self.state
    }

    /// Returns the assigned agent.
    #[must_use]
    pub const fn assigned_agent(&self) -> Option<AgentId> {
        self.assigned_agent
    }

    /// Returns the number of execution attempts that have begun.
    #[must_use]
    pub const fn attempts_started(&self) -> u16 {
        self.attempts_started
    }

    /// Returns successful output when completed.
    #[must_use]
    pub const fn result(&self) -> Option<&TaskResult> {
        self.result.as_ref()
    }

    /// Returns the most recent failure.
    #[must_use]
    pub const fn failure(&self) -> Option<&TaskFailure> {
        self.failure.as_ref()
    }

    /// Returns the optimistic-concurrency revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn set_state(&mut self, state: TaskState) -> Result<(), TaskStatusError> {
        self.bump_revision()?;
        self.state = state;
        Ok(())
    }

    pub(crate) fn assign(&mut self, agent_id: AgentId) -> Result<(), TaskStatusError> {
        self.bump_revision()?;
        self.assigned_agent = Some(agent_id);
        self.state = TaskState::Scheduled;
        Ok(())
    }

    pub(crate) fn start_attempt(&mut self) -> Result<(), TaskStatusError> {
        let attempts = self
            .attempts_started
            .checked_add(1)
            .ok_or(TaskStatusError::AttemptsExhausted)?;
        self.bump_revision()?;
        self.attempts_started = attempts;
        self.state = TaskState::Running;
        self.failure = None;
        Ok(())
    }

    pub(crate) fn complete(&mut self, result: TaskResult) -> Result<(), TaskStatusError> {
        self.bump_revision()?;
        self.state = TaskState::Completed;
        self.result = Some(result);
        self.failure = None;
        Ok(())
    }

    pub(crate) fn fail(&mut self, failure: TaskFailure) -> Result<(), TaskStatusError> {
        self.bump_revision()?;
        self.state = TaskState::Failed;
        self.result = None;
        self.failure = Some(failure);
        Ok(())
    }

    pub(crate) fn prepare_retry(&mut self) -> Result<(), TaskStatusError> {
        self.bump_revision()?;
        self.state = TaskState::Queued;
        self.assigned_agent = None;
        self.result = None;
        self.failure = None;
        Ok(())
    }

    fn bump_revision(&mut self) -> Result<(), TaskStatusError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(TaskStatusError::RevisionExhausted)?;
        Ok(())
    }

    fn validate(&self) -> Result<(), TaskStatusError> {
        if self.revision == 0 {
            return Err(TaskStatusError::InvalidSnapshot(
                "revision must be greater than zero",
            ));
        }
        let assignment_required = matches!(
            self.state,
            TaskState::Scheduled
                | TaskState::Running
                | TaskState::WaitingTool
                | TaskState::WaitingAgent
                | TaskState::WaitingApproval
                | TaskState::Completed
                | TaskState::Failed
        );
        if assignment_required && self.assigned_agent.is_none() {
            return Err(TaskStatusError::InvalidSnapshot(
                "state requires an assigned agent",
            ));
        }
        if matches!(
            self.state,
            TaskState::Running
                | TaskState::WaitingTool
                | TaskState::WaitingAgent
                | TaskState::WaitingApproval
                | TaskState::Completed
                | TaskState::Failed
        ) && self.attempts_started == 0
        {
            return Err(TaskStatusError::InvalidSnapshot(
                "execution state requires at least one attempt",
            ));
        }
        if (self.state == TaskState::Completed) != self.result.is_some() {
            return Err(TaskStatusError::InvalidSnapshot(
                "result presence does not match completed state",
            ));
        }
        if (self.state == TaskState::Failed) != self.failure.is_some() {
            return Err(TaskStatusError::InvalidSnapshot(
                "failure presence does not match failed state",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskStatusWire {
    task_id: TaskId,
    state: TaskState,
    #[serde(default)]
    assigned_agent: Option<AgentId>,
    attempts_started: u16,
    #[serde(default)]
    result: Option<TaskResult>,
    #[serde(default)]
    failure: Option<TaskFailure>,
    revision: u64,
}

impl<'de> Deserialize<'de> for TaskStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TaskStatusWire::deserialize(deserializer)?;
        let status = Self {
            task_id: wire.task_id,
            state: wire.state,
            assigned_agent: wire.assigned_agent,
            attempts_started: wire.attempts_started,
            result: wire.result,
            failure: wire.failure,
            revision: wire.revision,
        };
        status.validate().map_err(de::Error::custom)?;
        Ok(status)
    }
}

/// Internal status mutation or snapshot validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskStatusError {
    RevisionExhausted,
    AttemptsExhausted,
    InvalidSnapshot(&'static str),
}

impl fmt::Display for TaskStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionExhausted => formatter.write_str("task status revision is exhausted"),
            Self::AttemptsExhausted => formatter.write_str("task attempt counter is exhausted"),
            Self::InvalidSnapshot(message) => write!(formatter, "invalid task snapshot: {message}"),
        }
    }
}

impl Error for TaskStatusError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialization_rejects_running_task_without_assignment() {
        let status = TaskStatus::pending();
        let mut value = serde_json::to_value(status).unwrap();
        value["state"] = serde_json::json!("RUNNING");
        value["attemptsStarted"] = serde_json::json!(1);

        assert!(serde_json::from_value::<TaskStatus>(value).is_err());
    }

    #[test]
    fn total_tokens_reports_overflow() {
        let usage = TaskUsage::new(u64::MAX, 1, 0);
        assert_eq!(usage.total_tokens(), None);
    }

    #[test]
    fn completed_snapshot_requires_a_result() {
        let status = TaskStatus::pending();
        let mut value = serde_json::to_value(status).unwrap();
        value["state"] = serde_json::json!("COMPLETED");
        value["assignedAgent"] = serde_json::to_value(AgentId::new()).unwrap();
        value["attemptsStarted"] = serde_json::json!(1);

        assert!(serde_json::from_value::<TaskStatus>(value).is_err());
    }
}
