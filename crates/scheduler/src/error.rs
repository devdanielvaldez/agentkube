use agentkube_agents::ProviderName;
use agentkube_core::{AgentId, NodeId, ResourceUid};
use agentkube_tasks::TaskState;
use std::{error::Error, fmt};

/// Invalid normalized scheduler metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateError {
    /// A metric exceeded 10,000 basis points.
    MetricOutOfRange {
        /// Metric field.
        field: &'static str,
        /// Rejected value.
        value: u16,
    },
    /// The instance references a different definition.
    DefinitionMismatch {
        /// UID referenced by the instance.
        expected: ResourceUid,
        /// UID supplied by the definition.
        actual: ResourceUid,
    },
    /// The instance is assigned to a different node.
    NodeMismatch {
        /// Agent being evaluated.
        agent_id: AgentId,
        /// Node assigned to the instance.
        expected: Option<NodeId>,
        /// Supplied candidate node.
        actual: NodeId,
    },
    /// A node advertised the same provider more than once.
    DuplicateProvider(ProviderName),
    /// Used capacity exceeded total capacity.
    InvalidCapacity {
        /// Running slots.
        used: u16,
        /// Configured slots.
        total: u16,
    },
}

impl fmt::Display for CandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MetricOutOfRange { field, value } => {
                write!(formatter, "{field} must be at most 10000, received {value}")
            }
            Self::DefinitionMismatch { expected, actual } => write!(
                formatter,
                "instance references definition {expected}, but candidate supplied {actual}"
            ),
            Self::NodeMismatch {
                agent_id,
                expected,
                actual,
            } => write!(
                formatter,
                "agent {agent_id} is assigned to {expected:?}, not node {actual}"
            ),
            Self::DuplicateProvider(provider) => {
                write!(
                    formatter,
                    "node advertises provider {provider} more than once"
                )
            }
            Self::InvalidCapacity { used, total } => {
                write!(formatter, "node uses {used} slots but has capacity {total}")
            }
        }
    }
}

impl Error for CandidateError {}

/// Invalid scheduler scoring configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerConfigError {
    /// Component weights must add up to exactly 10,000 basis points.
    InvalidWeightTotal(u32),
}

impl fmt::Display for SchedulerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWeightTotal(total) => write!(
                formatter,
                "scheduler weights must total 10000 basis points, received {total}"
            ),
        }
    }
}

impl Error for SchedulerConfigError {}

/// Invalid scheduling request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingError {
    /// Only queued tasks may be placed.
    TaskNotQueued(TaskState),
    /// Arithmetic exceeded a supported scheduler counter.
    ArithmeticOverflow,
}

impl fmt::Display for SchedulingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskNotQueued(state) => {
                write!(
                    formatter,
                    "scheduler requires a QUEUED task, received {state:?}"
                )
            }
            Self::ArithmeticOverflow => {
                formatter.write_str("scheduler score arithmetic overflowed")
            }
        }
    }
}

impl Error for SchedulingError {}
