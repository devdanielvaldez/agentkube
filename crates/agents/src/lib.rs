//! Agent definitions, deployments, and runtime lifecycle state.
//!
//! This crate owns the AgentKube agent domain. It contains no persistence,
//! transport, provider SDK, or scheduler implementation.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod definition;
mod deployment;
mod instance;
mod model;
mod spec;
mod status;
mod value;

pub use definition::{AgentDefinition, AgentDocument, AgentDocumentError};
pub use deployment::{
    AgentDeployment, AgentDeploymentDocument, AgentDeploymentDocumentError, AgentDeploymentSpec,
    AgentDeploymentStatus, AgentSelector, DeploymentStatusError, ReplicaCount, ReplicaCountError,
    RestartPolicy,
};
pub use instance::{AgentInstance, AgentInstanceError, AgentInstanceState};
pub use model::{ModelPolicy, ModelPolicyError, OptimizationObjective};
pub use spec::{
    AgentPermissions, AgentResourceLimits, AgentSpec, FilesystemAccess, MemoryPolicy, NetworkAccess,
};
pub use status::{AgentCondition, AgentConditionType, AgentPhase, AgentStatus, ConditionStatus};
pub use value::{
    AgentRole, Capability, DomainValueError, DomainValueErrorKind, Instructions, ModelName,
    ProviderName, ToolName,
};
