//! Deterministic placement decisions for AgentKube tasks.
//!
//! Scheduling is deliberately infrastructure independent: callers provide a
//! consistent candidate snapshot, then persist the returned decision through
//! their own transactional control-plane adapter.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod candidate;
mod error;
mod scheduler;
mod score;

pub use candidate::{
    CandidateMetrics, NodeLocality, NodeSnapshot, ProviderAvailability, SchedulingCandidate,
};
pub use error::{CandidateError, SchedulerConfigError, SchedulingError};
pub use scheduler::{
    CandidateEvaluation, PlacementDecision, RejectionReason, ScheduleOutcome, Scheduler,
    UnschedulableReport,
};
pub use score::{BasisPoints, ScoreBreakdown, ScoreWeights};
