use crate::{RuntimeError, WorkerStateError};
use agentkube_queue::QueueError;
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Result returned by worker orchestration operations.
pub type WorkerResult<T> = Result<T, WorkerError>;

/// Sendable future returned by worker orchestration operations.
pub type WorkerFuture<'a, T> = Pin<Box<dyn Future<Output = WorkerResult<T>> + Send + 'a>>;

/// Infrastructure or coordination failure while processing work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    /// The worker has reached its configured execution capacity.
    AtCapacity,
    /// The heartbeat sequence counter is exhausted.
    HeartbeatSequenceExhausted,
    /// Runtime health could not be established.
    RuntimeHealth(RuntimeError),
    /// The task queue operation failed.
    Queue(QueueError),
    /// Atomic task/agent state coordination failed.
    State(WorkerStateError),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AtCapacity => formatter.write_str("worker is at execution capacity"),
            Self::HeartbeatSequenceExhausted => {
                formatter.write_str("worker heartbeat sequence is exhausted")
            }
            Self::RuntimeHealth(error) => write!(formatter, "runtime health check failed: {error}"),
            Self::Queue(error) => write!(formatter, "worker queue operation failed: {error}"),
            Self::State(error) => write!(formatter, "worker state operation failed: {error}"),
        }
    }
}

impl Error for WorkerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::AtCapacity | Self::HeartbeatSequenceExhausted => None,
            Self::RuntimeHealth(error) => Some(error),
            Self::Queue(error) => Some(error),
            Self::State(error) => Some(error),
        }
    }
}

impl From<QueueError> for WorkerError {
    fn from(value: QueueError) -> Self {
        Self::Queue(value)
    }
}

impl From<WorkerStateError> for WorkerError {
    fn from(value: WorkerStateError) -> Self {
        Self::State(value)
    }
}
