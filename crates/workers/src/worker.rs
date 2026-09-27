use crate::{
    AgentRuntime, FailureDisposition, RuntimeHealth, RuntimeOutput, WorkerError, WorkerFuture,
    WorkerResult, WorkerStateError, WorkerStateStore,
};
use agentkube_core::{AgentId, HumanDuration, NodeId, TaskId};
use agentkube_queue::{TaskLease, TaskQueue};
use agentkube_tasks::{TaskFailure, TaskState};
use std::{
    num::NonZeroU16,
    sync::{
        Arc,
        atomic::{AtomicU16, AtomicU64, Ordering},
    },
    time::SystemTime,
};

/// Worker concurrency and queue-lease settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerSettings {
    concurrency: NonZeroU16,
    lease_duration: HumanDuration,
}

impl WorkerSettings {
    /// Creates worker settings.
    #[must_use]
    pub const fn new(concurrency: NonZeroU16, lease_duration: HumanDuration) -> Self {
        Self {
            concurrency,
            lease_duration,
        }
    }

    /// Returns the maximum simultaneous executions.
    #[must_use]
    pub const fn concurrency(self) -> NonZeroU16 {
        self.concurrency
    }

    /// Returns the queue visibility lease duration.
    #[must_use]
    pub const fn lease_duration(self) -> HumanDuration {
        self.lease_duration
    }
}

/// Monotonic worker liveness observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerHeartbeat {
    node_id: NodeId,
    sequence: u64,
    observed_at: SystemTime,
    active_executions: u16,
    capacity: NonZeroU16,
}

impl WorkerHeartbeat {
    /// Returns the worker node identifier.
    #[must_use]
    pub const fn node_id(self) -> NodeId {
        self.node_id
    }

    /// Returns the one-based heartbeat sequence.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Returns when the heartbeat was observed locally.
    #[must_use]
    pub const fn observed_at(self) -> SystemTime {
        self.observed_at
    }

    /// Returns active task executions.
    #[must_use]
    pub const fn active_executions(self) -> u16 {
        self.active_executions
    }

    /// Returns configured execution capacity.
    #[must_use]
    pub const fn capacity(self) -> NonZeroU16 {
        self.capacity
    }
}

/// Reason a stale queue delivery was permanently discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    /// The queue referenced a task absent from worker state.
    TaskMissing,
    /// The task already left the queued state.
    TaskNotQueued(TaskState),
}

/// Result of one queue delivery handled by a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerRunOutcome {
    /// Execution completed and the queue delivery was acknowledged.
    Completed(RuntimeOutput),
    /// Execution failed but task retry policy requeued it.
    Requeued {
        /// Persisted normalized failure.
        failure: TaskFailure,
        /// Retry visibility delay.
        delay: Option<HumanDuration>,
    },
    /// Execution failed permanently and the queue delivery was acknowledged.
    Failed(TaskFailure),
    /// A stale queue delivery was acknowledged without execution.
    Discarded(DiscardReason),
}

/// Auditable report for one settled queue delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerReport {
    task_id: TaskId,
    delivery_attempt: u64,
    outcome: WorkerRunOutcome,
}

impl WorkerReport {
    fn new(task_id: TaskId, delivery_attempt: u64, outcome: WorkerRunOutcome) -> Self {
        Self {
            task_id,
            delivery_attempt,
            outcome,
        }
    }

    /// Returns the delivered task identifier.
    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Returns the queue delivery attempt number.
    #[must_use]
    pub const fn delivery_attempt(&self) -> u64 {
        self.delivery_attempt
    }

    /// Returns the settled execution outcome.
    #[must_use]
    pub const fn outcome(&self) -> &WorkerRunOutcome {
        &self.outcome
    }
}

/// Worker that atomically coordinates queue, state, and runtime execution.
pub struct Worker {
    node_id: NodeId,
    settings: WorkerSettings,
    queue: Arc<dyn TaskQueue>,
    state: Arc<dyn WorkerStateStore>,
    runtime: Arc<dyn AgentRuntime>,
    active: Arc<AtomicU16>,
    heartbeat_sequence: AtomicU64,
}

impl Worker {
    /// Creates a worker for one registered node.
    #[must_use]
    pub fn new(
        node_id: NodeId,
        settings: WorkerSettings,
        queue: Arc<dyn TaskQueue>,
        state: Arc<dyn WorkerStateStore>,
        runtime: Arc<dyn AgentRuntime>,
    ) -> Self {
        Self {
            node_id,
            settings,
            queue,
            state,
            runtime,
            active: Arc::new(AtomicU16::new(0)),
            heartbeat_sequence: AtomicU64::new(0),
        }
    }

    /// Returns the worker node identifier.
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// Creates a liveness observation with monotonic sequence numbering.
    pub fn heartbeat(&self) -> WorkerResult<WorkerHeartbeat> {
        let previous = self
            .heartbeat_sequence
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_add(1)
            })
            .map_err(|_| WorkerError::HeartbeatSequenceExhausted)?;
        Ok(WorkerHeartbeat {
            node_id: self.node_id,
            sequence: previous + 1,
            observed_at: SystemTime::now(),
            active_executions: self.active.load(Ordering::SeqCst),
            capacity: self.settings.concurrency,
        })
    }

    /// Leases and processes at most one task for a ready agent instance.
    pub fn run_once<'a>(&'a self, agent_id: AgentId) -> WorkerFuture<'a, Option<WorkerReport>> {
        Box::pin(async move {
            let _capacity = self.acquire_capacity()?;
            if self
                .runtime
                .health()
                .await
                .map_err(WorkerError::RuntimeHealth)?
                != RuntimeHealth::Healthy
            {
                return Err(WorkerError::State(WorkerStateError::Unavailable(
                    "agent runtime does not accept new work",
                )));
            }
            let Some(lease) = self
                .queue
                .lease(self.node_id, self.settings.lease_duration)
                .await?
            else {
                return Ok(None);
            };
            self.process_lease(agent_id, lease).await.map(Some)
        })
    }

    async fn process_lease(
        &self,
        agent_id: AgentId,
        lease: TaskLease,
    ) -> Result<WorkerReport, WorkerError> {
        let task_id = lease.task_id();
        let claim = match self.state.claim(task_id, agent_id, self.node_id).await {
            Ok(claim) => claim,
            Err(error) if error.discards_delivery() => {
                let reason = match error {
                    WorkerStateError::TaskNotFound(_) => DiscardReason::TaskMissing,
                    WorkerStateError::TaskNotQueued { actual, .. } => {
                        DiscardReason::TaskNotQueued(actual)
                    }
                    unexpected => {
                        self.queue
                            .release(lease.lease_id(), self.node_id, None)
                            .await?;
                        return Err(unexpected.into());
                    }
                };
                self.queue
                    .acknowledge(lease.lease_id(), self.node_id)
                    .await?;
                return Ok(WorkerReport::new(
                    task_id,
                    lease.delivery_attempt(),
                    WorkerRunOutcome::Discarded(reason),
                ));
            }
            Err(error) => {
                self.queue
                    .release(lease.lease_id(), self.node_id, None)
                    .await?;
                return Err(error.into());
            }
        };

        match self.runtime.execute(claim).await {
            Ok(output) => {
                self.state
                    .complete(task_id, agent_id, output.result().clone())
                    .await?;
                self.queue
                    .acknowledge(lease.lease_id(), self.node_id)
                    .await?;
                Ok(WorkerReport::new(
                    task_id,
                    lease.delivery_attempt(),
                    WorkerRunOutcome::Completed(output),
                ))
            }
            Err(error) => {
                let failure = TaskFailure::new(error.task_failure_kind(), error.to_string());
                let disposition = self.state.fail(task_id, agent_id, failure.clone()).await?;
                let outcome = match disposition {
                    FailureDisposition::Requeued { delay } => {
                        self.queue
                            .release(lease.lease_id(), self.node_id, delay)
                            .await?;
                        WorkerRunOutcome::Requeued { failure, delay }
                    }
                    FailureDisposition::Terminal => {
                        self.queue
                            .acknowledge(lease.lease_id(), self.node_id)
                            .await?;
                        WorkerRunOutcome::Failed(failure)
                    }
                };
                Ok(WorkerReport::new(
                    task_id,
                    lease.delivery_attempt(),
                    outcome,
                ))
            }
        }
    }

    fn acquire_capacity(&self) -> WorkerResult<CapacityGuard> {
        let maximum = self.settings.concurrency.get();
        self.active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |active| {
                (active < maximum).then_some(active + 1)
            })
            .map_err(|_| WorkerError::AtCapacity)?;
        Ok(CapacityGuard {
            active: Arc::clone(&self.active),
        })
    }
}

struct CapacityGuard {
    active: Arc<AtomicU16>,
}

impl Drop for CapacityGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
