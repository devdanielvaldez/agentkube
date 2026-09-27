use crate::{EnqueueRequest, LeaseId, QueueFuture, QueueStats, TaskLease};
use agentkube_core::{HumanDuration, NodeId, TaskId};

/// Asynchronous port for reliable task dispatch.
pub trait TaskQueue: Send + Sync {
    /// Adds a task, rejecting duplicate active entries.
    fn enqueue<'a>(&'a self, request: EnqueueRequest) -> QueueFuture<'a, ()>;

    /// Atomically leases the highest-priority visible task.
    fn lease<'a>(
        &'a self,
        consumer: NodeId,
        duration: HumanDuration,
    ) -> QueueFuture<'a, Option<TaskLease>>;

    /// Permanently removes a successfully handled delivery.
    fn acknowledge<'a>(&'a self, lease_id: LeaseId, consumer: NodeId) -> QueueFuture<'a, TaskId>;

    /// Returns a delivery to the queue, optionally after a delay.
    fn release<'a>(
        &'a self,
        lease_id: LeaseId,
        consumer: NodeId,
        delay: Option<HumanDuration>,
    ) -> QueueFuture<'a, TaskId>;

    /// Renews an active lease from the current queue time.
    fn extend<'a>(
        &'a self,
        lease_id: LeaseId,
        consumer: NodeId,
        duration: HumanDuration,
    ) -> QueueFuture<'a, TaskLease>;

    /// Returns a consistent occupancy snapshot after recovering expired leases.
    fn stats<'a>(&'a self) -> QueueFuture<'a, QueueStats>;
}
