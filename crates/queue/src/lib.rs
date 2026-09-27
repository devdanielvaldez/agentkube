//! Reliable task dispatch contracts and an in-memory queue backend.
//!
//! The queue transports task identities rather than complete task resources.
//! Durable task state remains owned by persistence, while queue leases provide
//! exclusive, recoverable delivery to workers.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod clock;
mod error;
mod memory;
mod model;
mod queue;

pub use clock::{ManualClock, QueueClock, SystemClock};
pub use error::{QueueError, QueueFuture, QueueResult};
pub use memory::InMemoryTaskQueue;
pub use model::{EnqueueRequest, EnqueueRequestError, LeaseId, QueueStats, TaskLease};
pub use queue::TaskQueue;
