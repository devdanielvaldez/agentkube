use crate::LeaseId;
use agentkube_core::{NodeId, TaskId};
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Result returned by task queue operations.
pub type QueueResult<T> = Result<T, QueueError>;

/// Sendable future returned by task queue operations.
pub type QueueFuture<'a, T> = Pin<Box<dyn Future<Output = QueueResult<T>> + Send + 'a>>;

/// Failure produced while dispatching or settling queued work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// The task already has an active queue entry.
    AlreadyQueued(TaskId),
    /// The lease is unknown or has expired.
    LeaseNotFound(LeaseId),
    /// A consumer attempted to mutate another consumer's lease.
    LeaseOwnerMismatch {
        /// Lease being accessed.
        lease_id: LeaseId,
        /// Consumer that owns the lease.
        expected: NodeId,
        /// Consumer supplied by the caller.
        actual: NodeId,
    },
    /// The task has exhausted its queue delivery-attempt counter.
    DeliveryAttemptsExhausted(TaskId),
    /// The stable FIFO sequence counter is exhausted.
    SequenceExhausted,
    /// A deadline or delayed-delivery timestamp could not be represented.
    TimeOverflow,
    /// The queue backend could not complete an operation.
    Unavailable(&'static str),
}

impl fmt::Display for QueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyQueued(task_id) => write!(formatter, "task {task_id} is already queued"),
            Self::LeaseNotFound(lease_id) => write!(formatter, "lease {lease_id} was not found"),
            Self::LeaseOwnerMismatch {
                lease_id,
                expected,
                actual,
            } => write!(
                formatter,
                "lease {lease_id} belongs to consumer {expected}, not {actual}"
            ),
            Self::DeliveryAttemptsExhausted(task_id) => {
                write!(formatter, "task {task_id} exhausted its delivery attempts")
            }
            Self::SequenceExhausted => formatter.write_str("queue sequence counter is exhausted"),
            Self::TimeOverflow => formatter.write_str("queue timestamp is outside supported range"),
            Self::Unavailable(reason) => write!(formatter, "queue is unavailable: {reason}"),
        }
    }
}

impl Error for QueueError {}
