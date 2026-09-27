use crate::{TaskFailure, TaskResult, TaskSpec, TaskState, TaskStatus, status::TaskStatusError};
use agentkube_core::{AgentId, HumanDuration, Metadata, Resource};
use agentkube_protocol::{ApiVersion, ResourceDocument, ResourceKind, TypeMeta};
use std::{error::Error, fmt, num::NonZeroU16};

/// Wire representation of an AgentTask resource.
pub type TaskDocument = ResourceDocument<TaskSpec, TaskStatus>;

/// Declarative task resource and its observed execution status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTask {
    metadata: Metadata,
    spec: TaskSpec,
    status: TaskStatus,
}

impl AgentTask {
    /// Current AgentTask API version.
    pub const API_VERSION: &'static str = "agentkube.ai/v1";
    /// Resource kind used on the wire.
    pub const KIND: &'static str = "AgentTask";

    /// Creates a pending task.
    #[must_use]
    pub fn new(metadata: Metadata, spec: TaskSpec) -> Self {
        Self {
            metadata,
            spec,
            status: TaskStatus::pending(),
        }
    }

    /// Reconstructs a task after validating its wire type metadata and status.
    pub fn from_document(document: TaskDocument) -> Result<Self, TaskDocumentError> {
        let (type_meta, metadata, spec, status) = document.into_parts();
        if type_meta.api_version().as_str() != Self::API_VERSION
            || type_meta.kind().as_str() != Self::KIND
        {
            return Err(TaskDocumentError {
                actual_api_version: type_meta.api_version().to_string(),
                actual_kind: type_meta.kind().to_string(),
            });
        }
        Ok(Self {
            metadata,
            spec,
            status: status.unwrap_or_else(TaskStatus::pending),
        })
    }

    /// Converts the task into its stable wire representation.
    #[must_use]
    pub fn into_document(self) -> TaskDocument {
        ResourceDocument::new(task_type_meta(), self.metadata, self.spec).with_status(self.status)
    }

    /// Submits a pending task to the scheduling queue.
    pub fn enqueue(&mut self) -> Result<(), TaskOperationError> {
        self.require_state(TaskState::Pending, "enqueue")?;
        self.set_state(TaskState::Queued)
    }

    /// Assigns a queued task to an agent.
    pub fn schedule(&mut self, agent_id: AgentId) -> Result<(), TaskOperationError> {
        self.require_state(TaskState::Queued, "schedule")?;
        self.status.assign(agent_id).map_err(Into::into)
    }

    /// Starts a scheduled execution attempt.
    pub fn start(&mut self) -> Result<(), TaskOperationError> {
        self.require_state(TaskState::Scheduled, "start")?;
        self.status.start_attempt().map_err(Into::into)
    }

    /// Pauses running execution while a tool call is outstanding.
    pub fn wait_for_tool(&mut self) -> Result<(), TaskOperationError> {
        self.enter_wait(TaskState::WaitingTool, "wait_for_tool")
    }

    /// Pauses running execution while another agent is outstanding.
    pub fn wait_for_agent(&mut self) -> Result<(), TaskOperationError> {
        self.enter_wait(TaskState::WaitingAgent, "wait_for_agent")
    }

    /// Pauses running execution for human approval.
    pub fn wait_for_approval(&mut self) -> Result<(), TaskOperationError> {
        self.enter_wait(TaskState::WaitingApproval, "wait_for_approval")
    }

    /// Resumes execution from any waiting state.
    pub fn resume(&mut self) -> Result<(), TaskOperationError> {
        if !self.status.state().is_waiting() {
            return Err(TaskOperationError::InvalidState {
                operation: "resume",
                expected: "a waiting state",
                actual: self.status.state(),
            });
        }
        self.set_state(TaskState::Running)
    }

    /// Completes a running task successfully.
    pub fn complete(&mut self, result: TaskResult) -> Result<(), TaskOperationError> {
        self.require_state(TaskState::Running, "complete")?;
        self.status.complete(result).map_err(Into::into)
    }

    /// Ends active execution with a structured failure.
    pub fn fail(&mut self, failure: TaskFailure) -> Result<(), TaskOperationError> {
        if !matches!(
            self.status.state(),
            TaskState::Running
                | TaskState::WaitingTool
                | TaskState::WaitingAgent
                | TaskState::WaitingApproval
        ) {
            return Err(TaskOperationError::InvalidState {
                operation: "fail",
                expected: "running or waiting",
                actual: self.status.state(),
            });
        }
        self.status.fail(failure).map_err(Into::into)
    }

    /// Requeues a retryable failed task when attempts remain.
    pub fn retry(&mut self) -> Result<(), TaskOperationError> {
        self.require_state(TaskState::Failed, "retry")?;
        let failure = self
            .status
            .failure()
            .ok_or(TaskOperationError::MissingFailure)?;
        if !self
            .spec
            .retry_policy()
            .permits(self.status.attempts_started(), failure.kind())
        {
            return Err(TaskOperationError::RetryNotAllowed);
        }
        self.status.prepare_retry().map_err(Into::into)
    }

    /// Returns the delay that would be applied before the next permitted retry.
    #[must_use]
    pub fn retry_delay(&self) -> Option<HumanDuration> {
        if self.status.state() != TaskState::Failed {
            return None;
        }
        let failure = self.status.failure()?;
        if !self
            .spec
            .retry_policy()
            .permits(self.status.attempts_started(), failure.kind())
        {
            return None;
        }
        let retry_number = NonZeroU16::new(self.status.attempts_started())?;
        self.spec
            .retry_policy()
            .backoff()
            .delay_for_retry(retry_number)
    }

    /// Cancels a non-terminal task.
    pub fn cancel(&mut self) -> Result<(), TaskOperationError> {
        if self.status.state().is_terminal() {
            return Err(TaskOperationError::AlreadyTerminal(self.status.state()));
        }
        self.set_state(TaskState::Cancelled)
    }

    /// Returns immutable desired task state.
    #[must_use]
    pub const fn spec(&self) -> &TaskSpec {
        &self.spec
    }

    /// Returns observed execution state.
    #[must_use]
    pub const fn status(&self) -> &TaskStatus {
        &self.status
    }

    fn enter_wait(
        &mut self,
        state: TaskState,
        operation: &'static str,
    ) -> Result<(), TaskOperationError> {
        self.require_state(TaskState::Running, operation)?;
        self.set_state(state)
    }

    fn require_state(
        &self,
        expected: TaskState,
        operation: &'static str,
    ) -> Result<(), TaskOperationError> {
        if self.status.state() == expected {
            Ok(())
        } else {
            Err(TaskOperationError::InvalidState {
                operation,
                expected: expected.description(),
                actual: self.status.state(),
            })
        }
    }

    fn set_state(&mut self, state: TaskState) -> Result<(), TaskOperationError> {
        self.status.set_state(state).map_err(Into::into)
    }
}

impl Resource for AgentTask {
    const API_VERSION: &'static str = Self::API_VERSION;
    const KIND: &'static str = Self::KIND;

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }
}

impl TaskState {
    const fn description(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Queued => "QUEUED",
            Self::Scheduled => "SCHEDULED",
            Self::Running => "RUNNING",
            Self::WaitingTool => "WAITING_TOOL",
            Self::WaitingAgent => "WAITING_AGENT",
            Self::WaitingApproval => "WAITING_APPROVAL",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        }
    }
}

/// Wire resource type mismatch while restoring an AgentTask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDocumentError {
    actual_api_version: String,
    actual_kind: String,
}

impl TaskDocumentError {
    /// Returns the received API version.
    #[must_use]
    pub fn actual_api_version(&self) -> &str {
        &self.actual_api_version
    }

    /// Returns the received resource kind.
    #[must_use]
    pub fn actual_kind(&self) -> &str {
        &self.actual_kind
    }
}

impl fmt::Display for TaskDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "expected {}/{}, received {}/{}",
            AgentTask::API_VERSION,
            AgentTask::KIND,
            self.actual_api_version,
            self.actual_kind
        )
    }
}

impl Error for TaskDocumentError {}

/// Invalid task lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOperationError {
    /// Operation was attempted from the wrong state.
    InvalidState {
        /// Stable operation name.
        operation: &'static str,
        /// Required state or state category.
        expected: &'static str,
        /// Actual task state.
        actual: TaskState,
    },
    /// A terminal task cannot be cancelled or executed again.
    AlreadyTerminal(TaskState),
    /// Retry policy does not permit another attempt for this failure.
    RetryNotAllowed,
    /// Failed state did not contain the required failure details.
    MissingFailure,
    /// Status revision cannot be incremented.
    RevisionExhausted,
    /// Attempt counter cannot be incremented.
    AttemptsExhausted,
}

impl From<TaskStatusError> for TaskOperationError {
    fn from(value: TaskStatusError) -> Self {
        match value {
            TaskStatusError::RevisionExhausted => Self::RevisionExhausted,
            TaskStatusError::AttemptsExhausted => Self::AttemptsExhausted,
            TaskStatusError::InvalidSnapshot(_) => {
                unreachable!("snapshot validation is not used during lifecycle mutations")
            }
        }
    }
}

impl fmt::Display for TaskOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidState {
                operation,
                expected,
                actual,
            } => write!(
                formatter,
                "operation {operation} requires {expected}, but task is {actual:?}"
            ),
            Self::AlreadyTerminal(state) => write!(formatter, "task is already {state:?}"),
            Self::RetryNotAllowed => formatter.write_str("task retry is not allowed"),
            Self::MissingFailure => formatter.write_str("failed task has no failure details"),
            Self::RevisionExhausted => formatter.write_str("task revision is exhausted"),
            Self::AttemptsExhausted => formatter.write_str("task attempt counter is exhausted"),
        }
    }
}

impl Error for TaskOperationError {}

fn task_type_meta() -> TypeMeta {
    TypeMeta::new(
        ApiVersion::new(AgentTask::API_VERSION).expect("static API version is valid"),
        ResourceKind::new(AgentTask::KIND).expect("static resource kind is valid"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackoffPolicy, Objective, RetryPolicy, TaskFailureKind, TaskUsage};
    use std::num::NonZeroU16;

    fn retryable_task() -> AgentTask {
        let policy = RetryPolicy::new(NonZeroU16::new(3).unwrap())
            .with_retry_on(TaskFailureKind::Timeout)
            .with_backoff(BackoffPolicy::Fixed {
                delay: "2s".parse().unwrap(),
            });
        AgentTask::new(
            Metadata::new("fix-login-bug").unwrap(),
            TaskSpec::new(Objective::new("Fix the login bug.").unwrap()).with_retry_policy(policy),
        )
    }

    fn running_task() -> AgentTask {
        let mut task = retryable_task();
        task.enqueue().unwrap();
        task.schedule(AgentId::new()).unwrap();
        task.start().unwrap();
        task
    }

    #[test]
    fn task_runs_waits_resumes_and_completes() {
        let mut task = running_task();
        task.wait_for_tool().unwrap();
        task.resume().unwrap();
        task.complete(TaskResult::new("fixed", TaskUsage::new(10, 5, 200)))
            .unwrap();

        assert_eq!(task.status().state(), TaskState::Completed);
        assert_eq!(task.status().result().unwrap().output(), "fixed");
    }

    #[test]
    fn retry_respects_failure_kind_and_attempt_ceiling() {
        let mut task = running_task();
        task.fail(TaskFailure::new(TaskFailureKind::Timeout, "timed out"))
            .unwrap();
        assert_eq!(task.retry_delay().unwrap().to_string(), "2s");
        task.retry().unwrap();

        assert_eq!(task.status().state(), TaskState::Queued);
        assert_eq!(task.status().assigned_agent(), None);
    }

    #[test]
    fn non_retryable_failure_stays_failed() {
        let mut task = running_task();
        task.fail(TaskFailure::new(
            TaskFailureKind::PermissionDenied,
            "forbidden",
        ))
        .unwrap();

        assert_eq!(task.retry(), Err(TaskOperationError::RetryNotAllowed));
        assert_eq!(task.status().state(), TaskState::Failed);
    }

    #[test]
    fn document_round_trip_preserves_task_identity_and_state() {
        let original = running_task();
        let id = original.status().task_id();
        let json = serde_json::to_string(&original.clone().into_document()).unwrap();
        let document: TaskDocument = serde_json::from_str(&json).unwrap();
        let decoded = AgentTask::from_document(document).unwrap();

        assert_eq!(decoded, original);
        assert_eq!(decoded.status().task_id(), id);
    }

    #[test]
    fn retry_stops_at_the_total_attempt_ceiling() {
        let policy =
            RetryPolicy::new(NonZeroU16::new(2).unwrap()).with_retry_on(TaskFailureKind::Timeout);
        let mut task = AgentTask::new(
            Metadata::new("bounded-retry").unwrap(),
            TaskSpec::new(Objective::new("Retry at most once.").unwrap()).with_retry_policy(policy),
        );

        task.enqueue().unwrap();
        task.schedule(AgentId::new()).unwrap();
        task.start().unwrap();
        task.fail(TaskFailure::new(TaskFailureKind::Timeout, "first"))
            .unwrap();
        task.retry().unwrap();
        task.schedule(AgentId::new()).unwrap();
        task.start().unwrap();
        task.fail(TaskFailure::new(TaskFailureKind::Timeout, "second"))
            .unwrap();

        assert_eq!(task.retry(), Err(TaskOperationError::RetryNotAllowed));
        assert_eq!(task.status().attempts_started(), 2);
    }

    #[test]
    fn terminal_task_cannot_be_cancelled() {
        let mut task = running_task();
        task.complete(TaskResult::new("done", TaskUsage::default()))
            .unwrap();

        assert_eq!(
            task.cancel(),
            Err(TaskOperationError::AlreadyTerminal(TaskState::Completed))
        );
    }

    #[test]
    fn document_with_wrong_kind_is_rejected() {
        let original = retryable_task();
        let (_, metadata, spec, status) = original.into_document().into_parts();
        let type_meta = TypeMeta::new(
            ApiVersion::new(AgentTask::API_VERSION).unwrap(),
            ResourceKind::new("Agent").unwrap(),
        );
        let document =
            ResourceDocument::new(type_meta, metadata, spec).with_status(status.unwrap());

        assert!(AgentTask::from_document(document).is_err());
    }
}
