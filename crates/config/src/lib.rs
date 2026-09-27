//! Typed, layered runtime configuration for AgentKube processes.
//!
//! Configuration sources are applied in registration order, so later sources
//! override earlier ones. Every resulting configuration is validated before it
//! can be returned to an application.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod loader;
mod model;
mod source;

pub use agentkube_core::{DurationError, DurationErrorKind, HumanDuration};
pub use error::{ConfigError, ConfigValidationError};
pub use loader::ConfigLoader;
pub use model::{
    AgentKubeConfig, AuthConfig, AuthConfigPatch, ConfigPatch, ControlPlaneConfig,
    ControlPlaneConfigPatch, LogLevel, OperatorConfig, OperatorConfigPatch, ProvidersConfig,
    ProvidersConfigPatch, RuntimeEnvironment, SamplingRatio, SamplingRatioError, SecretString,
    StorageConfig, StorageConfigPatch, TelemetryConfig, TelemetryConfigPatch, WorkerConfig,
    WorkerConfigPatch,
};
pub use source::{ConfigSource, EnvironmentSource, JsonFileSource, JsonSource};
