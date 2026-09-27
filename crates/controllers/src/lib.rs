//! Idempotent desired-state reconciliation for AgentKube resources.
//!
//! Reconcilers are deterministic and side-effect free. They return declarative
//! actions plus an optional status update, allowing control-plane adapters to
//! persist state and execute actions with optimistic concurrency.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod definition;
mod deployment;
mod error;
mod outcome;

pub use definition::AgentDefinitionReconciler;
pub use deployment::{AgentDeploymentReconciler, DeploymentAction, ManagedAgentInstance};
pub use error::{ControllerError, ControllerResult};
pub use outcome::{ReconcilePlan, ReconcileState};
