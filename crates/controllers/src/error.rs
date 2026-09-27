use agentkube_agents::DeploymentStatusError;
use agentkube_core::{AgentId, ResourceVersionError};
use std::{error::Error, fmt};

/// Result returned by AgentKube reconcilers.
pub type ControllerResult<T> = Result<T, ControllerError>;

/// Invalid snapshot or domain mutation encountered during reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    /// The same agent instance appeared more than once in one snapshot.
    DuplicateAgent(AgentId),
    /// A status update violated deployment-domain invariants.
    DeploymentStatus(DeploymentStatusError),
    /// Persisting a status update cannot advance its resource version.
    ResourceVersion(ResourceVersionError),
    /// A replica count could not be represented by the deployment domain.
    ReplicaCountOverflow,
}

impl fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateAgent(id) => write!(formatter, "agent {id} appears twice in snapshot"),
            Self::DeploymentStatus(error) => {
                write!(formatter, "invalid deployment status: {error}")
            }
            Self::ResourceVersion(error) => {
                write!(formatter, "cannot advance status version: {error}")
            }
            Self::ReplicaCountOverflow => {
                formatter.write_str("observed replica count exceeds supported range")
            }
        }
    }
}

impl Error for ControllerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::DeploymentStatus(error) => Some(error),
            Self::ResourceVersion(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DeploymentStatusError> for ControllerError {
    fn from(value: DeploymentStatusError) -> Self {
        Self::DeploymentStatus(value)
    }
}

impl From<ResourceVersionError> for ControllerError {
    fn from(value: ResourceVersionError) -> Self {
        Self::ResourceVersion(value)
    }
}
