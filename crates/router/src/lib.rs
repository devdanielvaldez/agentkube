//! Deterministic model routing, provider selection, and failover.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod model;
mod registry;
mod router;

pub use error::{RegistryError, RouterError, RouterFuture, RouterResult};
pub use model::{
    DataResidency, ModelRoutingProfile, RouteDecision, RoutedResponse, RoutedStream,
    RoutingConstraints, RoutingProfileError, RoutingRequest,
};
pub use registry::ProviderRegistry;
pub use router::{ModelRouter, ModelRouting};
