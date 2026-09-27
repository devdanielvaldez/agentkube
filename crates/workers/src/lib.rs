//! Worker execution, runtime integration, heartbeats, and atomic task state.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod runtime;
mod state;
mod worker;

pub use error::{WorkerError, WorkerFuture, WorkerResult};
pub use runtime::{
    AgentRuntime, RuntimeError, RuntimeFuture, RuntimeHealth, RuntimeOutput, SingleTurnRuntime,
};
pub use state::{
    ExecutionClaim, FailureDisposition, InMemoryWorkerStateStore, WorkerStateError,
    WorkerStateFuture, WorkerStateStore,
};
pub use worker::{
    DiscardReason, Worker, WorkerHeartbeat, WorkerReport, WorkerRunOutcome, WorkerSettings,
};
