use crate::{ConfigValidationError, HumanDuration};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt, net::SocketAddr, num::NonZeroU16};

const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WORKER_CONCURRENCY: u16 = 4096;

/// Fully resolved and validated configuration for an AgentKube process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentKubeConfig {
    environment: RuntimeEnvironment,
    control_plane: ControlPlaneConfig,
    worker: WorkerConfig,
    telemetry: TelemetryConfig,
    shutdown_grace_period: HumanDuration,
}

impl AgentKubeConfig {
    /// Returns the runtime environment.
    #[must_use]
    pub const fn environment(&self) -> RuntimeEnvironment {
        self.environment
    }

    /// Returns control-plane server settings.
    #[must_use]
    pub const fn control_plane(&self) -> &ControlPlaneConfig {
        &self.control_plane
    }

    /// Returns worker lifecycle settings.
    #[must_use]
    pub const fn worker(&self) -> &WorkerConfig {
        &self.worker
    }

    /// Returns telemetry settings.
    #[must_use]
    pub const fn telemetry(&self) -> &TelemetryConfig {
        &self.telemetry
    }

    /// Returns the graceful shutdown deadline.
    #[must_use]
    pub const fn shutdown_grace_period(&self) -> HumanDuration {
        self.shutdown_grace_period
    }

    /// Validates invariants involving multiple configuration fields.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        if self.worker.concurrency.get() > MAX_WORKER_CONCURRENCY {
            return Err(ConfigValidationError::new(
                "worker.concurrency",
                "must not exceed 4096",
            ));
        }
        if self.worker.heartbeat_interval >= self.worker.lease_timeout {
            return Err(ConfigValidationError::new(
                "worker.heartbeatInterval",
                "must be shorter than worker.leaseTimeout",
            ));
        }
        if self.control_plane.max_request_body_bytes < 1024 {
            return Err(ConfigValidationError::new(
                "controlPlane.maxRequestBodyBytes",
                "must be at least 1024 bytes",
            ));
        }
        if self.telemetry.service_name.trim().is_empty() {
            return Err(ConfigValidationError::new(
                "telemetry.serviceName",
                "must not be empty",
            ));
        }
        if self.telemetry.service_name.len() > 63 {
            return Err(ConfigValidationError::new(
                "telemetry.serviceName",
                "must not exceed 63 bytes",
            ));
        }
        Ok(())
    }

    pub(crate) fn apply(&mut self, patch: ConfigPatch) {
        if let Some(environment) = patch.environment {
            self.environment = environment;
        }
        if let Some(control_plane) = patch.control_plane {
            self.control_plane.apply(control_plane);
        }
        if let Some(worker) = patch.worker {
            self.worker.apply(worker);
        }
        if let Some(telemetry) = patch.telemetry {
            self.telemetry.apply(telemetry);
        }
        if let Some(shutdown_grace_period) = patch.shutdown_grace_period {
            self.shutdown_grace_period = shutdown_grace_period;
        }
    }
}

impl Default for AgentKubeConfig {
    fn default() -> Self {
        Self {
            environment: RuntimeEnvironment::Development,
            control_plane: ControlPlaneConfig::default(),
            worker: WorkerConfig::default(),
            telemetry: TelemetryConfig::default(),
            shutdown_grace_period: "30s".parse().expect("default duration is valid"),
        }
    }
}

/// Environment in which an AgentKube process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeEnvironment {
    /// Local development with human-readable defaults.
    Development,
    /// Automated test execution.
    Test,
    /// Production execution.
    Production,
}

/// HTTP server settings for the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlPlaneConfig {
    bind_address: SocketAddr,
    request_timeout: HumanDuration,
    max_request_body_bytes: u64,
}

impl ControlPlaneConfig {
    /// Returns the socket used by the API server.
    #[must_use]
    pub const fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    /// Returns the complete request deadline.
    #[must_use]
    pub const fn request_timeout(&self) -> HumanDuration {
        self.request_timeout
    }

    /// Returns the largest accepted request body.
    #[must_use]
    pub const fn max_request_body_bytes(&self) -> u64 {
        self.max_request_body_bytes
    }

    fn apply(&mut self, patch: ControlPlaneConfigPatch) {
        if let Some(value) = patch.bind_address {
            self.bind_address = value;
        }
        if let Some(value) = patch.request_timeout {
            self.request_timeout = value;
        }
        if let Some(value) = patch.max_request_body_bytes {
            self.max_request_body_bytes = value;
        }
    }
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:8080".parse().expect("default socket is valid"),
            request_timeout: "30s".parse().expect("default duration is valid"),
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        }
    }
}

/// Worker concurrency and liveness settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerConfig {
    concurrency: NonZeroU16,
    heartbeat_interval: HumanDuration,
    lease_timeout: HumanDuration,
}

impl WorkerConfig {
    /// Returns the maximum number of concurrent task executions.
    #[must_use]
    pub const fn concurrency(&self) -> NonZeroU16 {
        self.concurrency
    }

    /// Returns the interval between worker heartbeats.
    #[must_use]
    pub const fn heartbeat_interval(&self) -> HumanDuration {
        self.heartbeat_interval
    }

    /// Returns the time after which a missing worker is considered unavailable.
    #[must_use]
    pub const fn lease_timeout(&self) -> HumanDuration {
        self.lease_timeout
    }

    fn apply(&mut self, patch: WorkerConfigPatch) {
        if let Some(value) = patch.concurrency {
            self.concurrency = value;
        }
        if let Some(value) = patch.heartbeat_interval {
            self.heartbeat_interval = value;
        }
        if let Some(value) = patch.lease_timeout {
            self.lease_timeout = value;
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: NonZeroU16::new(4).expect("default concurrency is non-zero"),
            heartbeat_interval: "10s".parse().expect("default duration is valid"),
            lease_timeout: "30s".parse().expect("default duration is valid"),
        }
    }
}

/// Logging and distributed-tracing settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetryConfig {
    service_name: String,
    log_level: LogLevel,
    json_logs: bool,
    trace_sample_ratio: SamplingRatio,
}

impl TelemetryConfig {
    /// Returns the OpenTelemetry service name.
    #[must_use]
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Returns the minimum emitted log level.
    #[must_use]
    pub const fn log_level(&self) -> LogLevel {
        self.log_level
    }

    /// Returns whether logs use a structured JSON representation.
    #[must_use]
    pub const fn json_logs(&self) -> bool {
        self.json_logs
    }

    /// Returns the fraction of traces that should be sampled.
    #[must_use]
    pub const fn trace_sample_ratio(&self) -> SamplingRatio {
        self.trace_sample_ratio
    }

    fn apply(&mut self, patch: TelemetryConfigPatch) {
        if let Some(value) = patch.service_name {
            self.service_name = value;
        }
        if let Some(value) = patch.log_level {
            self.log_level = value;
        }
        if let Some(value) = patch.json_logs {
            self.json_logs = value;
        }
        if let Some(value) = patch.trace_sample_ratio {
            self.trace_sample_ratio = value;
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            service_name: "agentkube".to_owned(),
            log_level: LogLevel::Info,
            json_logs: false,
            trace_sample_ratio: SamplingRatio::FULL,
        }
    }
}

/// Supported logging verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Highly detailed diagnostic events.
    Trace,
    /// Developer-oriented diagnostic events.
    Debug,
    /// Normal operational events.
    Info,
    /// Recoverable or suspicious conditions.
    Warn,
    /// Failures requiring attention.
    Error,
}

/// Validated trace sampling fraction in the inclusive range `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SamplingRatio(f64);

impl SamplingRatio {
    /// Sample no traces.
    pub const NONE: Self = Self(0.0);
    /// Sample every trace.
    pub const FULL: Self = Self(1.0);

    /// Creates a finite ratio in the inclusive range `0.0..=1.0`.
    pub fn new(value: f64) -> Result<Self, SamplingRatioError> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(SamplingRatioError(value))
        }
    }

    /// Returns the sampling fraction.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for SamplingRatio {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Error returned for an invalid trace sampling ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplingRatioError(f64);

impl SamplingRatioError {
    /// Returns the rejected ratio.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.0
    }
}

impl fmt::Display for SamplingRatioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sampling ratio {} is outside 0.0..=1.0", self.0)
    }
}

impl Error for SamplingRatioError {}

/// Partial configuration applied over existing values.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigPatch {
    pub(crate) environment: Option<RuntimeEnvironment>,
    pub(crate) control_plane: Option<ControlPlaneConfigPatch>,
    pub(crate) worker: Option<WorkerConfigPatch>,
    pub(crate) telemetry: Option<TelemetryConfigPatch>,
    pub(crate) shutdown_grace_period: Option<HumanDuration>,
}

impl ConfigPatch {
    /// Overrides the runtime environment.
    #[must_use]
    pub const fn with_environment(mut self, environment: RuntimeEnvironment) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Applies control-plane overrides.
    #[must_use]
    pub fn with_control_plane(mut self, control_plane: ControlPlaneConfigPatch) -> Self {
        self.control_plane = Some(control_plane);
        self
    }

    /// Applies worker overrides.
    #[must_use]
    pub fn with_worker(mut self, worker: WorkerConfigPatch) -> Self {
        self.worker = Some(worker);
        self
    }

    /// Applies telemetry overrides.
    #[must_use]
    pub fn with_telemetry(mut self, telemetry: TelemetryConfigPatch) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Overrides the graceful shutdown deadline.
    #[must_use]
    pub const fn with_shutdown_grace_period(mut self, duration: HumanDuration) -> Self {
        self.shutdown_grace_period = Some(duration);
        self
    }
}

/// Partial control-plane configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlPlaneConfigPatch {
    pub(crate) bind_address: Option<SocketAddr>,
    pub(crate) request_timeout: Option<HumanDuration>,
    pub(crate) max_request_body_bytes: Option<u64>,
}

impl ControlPlaneConfigPatch {
    /// Overrides the API server bind address.
    #[must_use]
    pub const fn with_bind_address(mut self, address: SocketAddr) -> Self {
        self.bind_address = Some(address);
        self
    }

    /// Overrides the request deadline.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: HumanDuration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// Overrides the maximum accepted request body size.
    #[must_use]
    pub const fn with_max_request_body_bytes(mut self, bytes: u64) -> Self {
        self.max_request_body_bytes = Some(bytes);
        self
    }
}

/// Partial worker configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerConfigPatch {
    pub(crate) concurrency: Option<NonZeroU16>,
    pub(crate) heartbeat_interval: Option<HumanDuration>,
    pub(crate) lease_timeout: Option<HumanDuration>,
}

impl WorkerConfigPatch {
    /// Overrides maximum task concurrency.
    #[must_use]
    pub const fn with_concurrency(mut self, concurrency: NonZeroU16) -> Self {
        self.concurrency = Some(concurrency);
        self
    }

    /// Overrides the worker heartbeat interval.
    #[must_use]
    pub const fn with_heartbeat_interval(mut self, interval: HumanDuration) -> Self {
        self.heartbeat_interval = Some(interval);
        self
    }

    /// Overrides the worker lease deadline.
    #[must_use]
    pub const fn with_lease_timeout(mut self, timeout: HumanDuration) -> Self {
        self.lease_timeout = Some(timeout);
        self
    }
}

/// Partial telemetry configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetryConfigPatch {
    pub(crate) service_name: Option<String>,
    pub(crate) log_level: Option<LogLevel>,
    pub(crate) json_logs: Option<bool>,
    pub(crate) trace_sample_ratio: Option<SamplingRatio>,
}

impl TelemetryConfigPatch {
    /// Overrides the OpenTelemetry service name.
    #[must_use]
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = Some(name.into());
        self
    }

    /// Overrides the minimum log level.
    #[must_use]
    pub const fn with_log_level(mut self, level: LogLevel) -> Self {
        self.log_level = Some(level);
        self
    }

    /// Enables or disables structured JSON logs.
    #[must_use]
    pub const fn with_json_logs(mut self, enabled: bool) -> Self {
        self.json_logs = Some(enabled);
        self
    }

    /// Overrides the distributed trace sampling ratio.
    #[must_use]
    pub const fn with_trace_sample_ratio(mut self, ratio: SamplingRatio) -> Self {
        self.trace_sample_ratio = Some(ratio);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_local() {
        let config = AgentKubeConfig::default();

        assert_eq!(config.environment(), RuntimeEnvironment::Development);
        assert_eq!(
            config.control_plane().bind_address().to_string(),
            "127.0.0.1:8080"
        );
        assert_eq!(config.worker().concurrency().get(), 4);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn heartbeat_must_be_shorter_than_lease() {
        let mut config = AgentKubeConfig::default();
        config.apply(ConfigPatch {
            worker: Some(WorkerConfigPatch {
                heartbeat_interval: Some("30s".parse().unwrap()),
                lease_timeout: Some("30s".parse().unwrap()),
                ..WorkerConfigPatch::default()
            }),
            ..ConfigPatch::default()
        });

        let error = config.validate().unwrap_err();
        assert_eq!(error.field(), "worker.heartbeatInterval");
    }

    #[test]
    fn sampling_ratio_validation_survives_deserialization() {
        assert!(serde_json::from_str::<SamplingRatio>("1.1").is_err());
    }
}
