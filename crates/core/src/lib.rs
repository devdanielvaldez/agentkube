//! Shared domain primitives for AgentKube.
//!
//! This crate contains stable, infrastructure-independent types used by the
//! rest of the workspace. Domain-specific behavior belongs in its respective
//! crate rather than here.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod id;
mod metadata;
mod name;
mod resource;
mod version;

pub use id::{AgentId, ModelId, NodeId, ProviderId, ResourceUid, TaskId, TraceId, WorkflowId};
pub use metadata::Metadata;
pub use name::{Namespace, ResourceName, ValidationError};
pub use resource::Resource;
pub use version::{ResourceVersion, ResourceVersionError};
