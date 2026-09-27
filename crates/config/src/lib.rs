//! Typed, layered runtime configuration for AgentKube processes.
//!
//! Configuration sources are applied in registration order, so later sources
//! override earlier ones. Every resulting configuration is validated before it
//! can be returned to an application.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod duration;
mod error;
mod loader;
mod model;
mod source;

pub use duration::{DurationError, DurationErrorKind, HumanDuration};
pub use error::{ConfigError, ConfigValidationError};
pub use loader::ConfigLoader;
pub use model::{
    AgentKubeConfig, ConfigPatch, ControlPlaneConfig, ControlPlaneConfigPatch, LogLevel,
    RuntimeEnvironment, SamplingRatio, SamplingRatioError, TelemetryConfig, TelemetryConfigPatch,
    WorkerConfig, WorkerConfigPatch,
};
pub use source::{ConfigSource, EnvironmentSource, JsonFileSource, JsonSource};
