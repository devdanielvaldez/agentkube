use agentkube_core::{HumanDuration, NodeId, TaskId};
use agentkube_tasks::{AgentTask, TaskPriority, TaskState};
use std::{error::Error, fmt, str::FromStr, time::SystemTime};
use uuid::Uuid;

/// Opaque, unguessable identifier authorizing a leased-task operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeaseId(Uuid);

impl LeaseId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the underlying UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for LeaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for LeaseId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Validated request to place a task in the dispatch queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnqueueRequest {
    task_id: TaskId,
    priority: TaskPriority,
    delay: Option<HumanDuration>,
}

impl EnqueueRequest {
    /// Creates an immediate request from an already-queued task resource.
    pub fn from_task(task: &AgentTask) -> Result<Self, EnqueueRequestError> {
        if task.status().state() != TaskState::Queued {
            return Err(EnqueueRequestError::TaskNotQueued(task.status().state()));
        }
        Ok(Self {
            task_id: task.status().task_id(),
            priority: task.spec().priority(),
            delay: None,
        })
    }

    /// Delays initial visibility, for example while applying retry backoff.
    #[must_use]
    pub const fn with_delay(mut self, delay: HumanDuration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Returns the queued task identifier.
    #[must_use]
    pub const fn task_id(self) -> TaskId {
        self.task_id
    }

    /// Returns the scheduling priority captured at enqueue time.
    #[must_use]
    pub const fn priority(self) -> TaskPriority {
        self.priority
    }

    /// Returns the optional initial visibility delay.
    #[must_use]
    pub const fn delay(self) -> Option<HumanDuration> {
        self.delay
    }
}

/// Invalid conversion from a task resource to an enqueue request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueRequestError {
    /// The task has not entered the queued lifecycle state.
    TaskNotQueued(TaskState),
}

impl fmt::Display for EnqueueRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskNotQueued(state) => {
                write!(
                    formatter,
                    "task must be QUEUED before enqueue, received {state:?}"
                )
            }
        }
    }
}

impl Error for EnqueueRequestError {}

/// Exclusive delivery of one task to a queue consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskLease {
    lease_id: LeaseId,
    task_id: TaskId,
    consumer: NodeId,
    priority: TaskPriority,
    delivery_attempt: u64,
    deadline: SystemTime,
}

impl TaskLease {
    pub(crate) const fn new(
        lease_id: LeaseId,
        task_id: TaskId,
        consumer: NodeId,
        priority: TaskPriority,
        delivery_attempt: u64,
        deadline: SystemTime,
    ) -> Self {
        Self {
            lease_id,
            task_id,
            consumer,
            priority,
            delivery_attempt,
            deadline,
        }
    }

    /// Returns the lease token required to settle this delivery.
    #[must_use]
    pub const fn lease_id(self) -> LeaseId {
        self.lease_id
    }

    /// Returns the delivered task identifier.
    #[must_use]
    pub const fn task_id(self) -> TaskId {
        self.task_id
    }

    /// Returns the node holding the lease.
    #[must_use]
    pub const fn consumer(self) -> NodeId {
        self.consumer
    }

    /// Returns the task priority captured at enqueue time.
    #[must_use]
    pub const fn priority(self) -> TaskPriority {
        self.priority
    }

    /// Returns the one-based delivery attempt for this queue entry.
    #[must_use]
    pub const fn delivery_attempt(self) -> u64 {
        self.delivery_attempt
    }

    /// Returns when the exclusive lease expires.
    #[must_use]
    pub const fn deadline(self) -> SystemTime {
        self.deadline
    }
}

/// Consistent snapshot of queue occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueStats {
    ready: usize,
    delayed: usize,
    leased: usize,
}

impl QueueStats {
    pub(crate) const fn new(ready: usize, delayed: usize, leased: usize) -> Self {
        Self {
            ready,
            delayed,
            leased,
        }
    }

    /// Returns tasks immediately available for delivery.
    #[must_use]
    pub const fn ready(self) -> usize {
        self.ready
    }

    /// Returns tasks waiting for their visibility time.
    #[must_use]
    pub const fn delayed(self) -> usize {
        self.delayed
    }

    /// Returns tasks currently leased to consumers.
    #[must_use]
    pub const fn leased(self) -> usize {
        self.leased
    }

    /// Returns all active entries in the queue.
    #[must_use]
    pub const fn total(self) -> usize {
        self.ready + self.delayed + self.leased
    }
}
