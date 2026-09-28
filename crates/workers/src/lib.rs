//! Worker execution, runtime integration, heartbeats, and atomic task state.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod repository;
mod runtime;
mod state;
mod tool;
mod worker;

pub use error::{WorkerError, WorkerFuture, WorkerResult};
pub use repository::RepositoryWorkerStateStore;
pub use runtime::{
    AgentRuntime, AgenticRuntime, RuntimeError, RuntimeFuture, RuntimeHealth, RuntimeOutput,
    SingleTurnRuntime,
};
pub use state::{
    ExecutionClaim, FailureDisposition, InMemoryWorkerStateStore, WorkerStateError,
    WorkerStateFuture, WorkerStateStore,
};
pub use tool::{ToolExecutionError, ToolExecutor, ToolFuture, ToolRegistry, ToolRegistryError};
pub use worker::{
    DiscardReason, Worker, WorkerHeartbeat, WorkerReport, WorkerRunOutcome, WorkerSettings,
};
