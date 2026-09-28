//! Local-first AgentKube operator.
//!
//! The operator serves the versioned HTTP API on durable SQLite storage and,
//! in the same process, runs the control loops that the idle API server does
//! not: crash recovery at boot, agent-status reconciliation on a tick, and
//! scheduled task dispatch with real model execution through the provider
//! adapters configured in [`AgentKubeConfig`](agentkube_config::AgentKubeConfig).
//!
//! Dispatch is sequential and deterministic: each cycle lists queued tasks by
//! priority, builds scheduling candidates from live instances, places each
//! task with the real [`Scheduler`](agentkube_scheduler::Scheduler), and
//! executes the winner. Unschedulable tasks stay queued with a visible
//! warning; nothing is force-assigned or synthesized.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod catalogs;
mod dispatch;
mod error;
mod reconcile;
mod recovery;

pub use catalogs::{CatalogSet, build_catalogs, discover_ollama_models};
pub use dispatch::{DispatchSummary, Dispatcher, ProviderDescriptor};
pub use error::OperatorError;
pub use reconcile::{ReconcileSummary, reconcile_once};
pub use recovery::{RecoverySummary, recover};
