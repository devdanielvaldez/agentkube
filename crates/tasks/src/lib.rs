//! Task specifications, scheduling requirements, retry policy, and lifecycle.
//!
//! This crate defines task-domain invariants without implementing queues,
//! persistence, scheduling, or worker execution.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod requirements;
mod retry;
mod spec;
mod status;
mod task;
mod value;

pub use requirements::{PrivacyRequirement, TaskRequirements, TaskRequirementsError};
pub use retry::{BackoffPolicy, BackoffPolicyError, RetryPolicy};
pub use spec::{TaskBudget, TaskPriority, TaskSpec};
pub use status::{TaskFailure, TaskFailureKind, TaskResult, TaskState, TaskStatus, TaskUsage};
pub use task::{AgentTask, TaskDocument, TaskDocumentError, TaskOperationError};
pub use value::{Objective, TaskValueError, TaskValueErrorKind};
