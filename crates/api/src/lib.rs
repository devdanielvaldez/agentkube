//! Production-oriented HTTP transport for the AgentKube control plane.
//!
//! The API exposes versioned resource endpoints while keeping storage and
//! queue implementations behind existing infrastructure-independent ports.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod handlers;
mod metrics;
mod nodes;
mod pagination;
mod server;
mod state;

pub use error::HttpApiError;
pub use nodes::{NodeInfo, NodeRegistry};
pub use server::{router, serve};
pub use state::ApiState;
