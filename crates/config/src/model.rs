use crate::{ConfigValidationError, HumanDuration};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt, net::SocketAddr, num::NonZeroU16, path::PathBuf, str::FromStr};

const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WORKER_CONCURRENCY: u16 = 4096;

/// Fully resolved and validated configuration for an AgentKube process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentKubeConfig {
    environment: RuntimeEnvironment,
    control_plane: ControlPlaneConfig,
    worker: WorkerConfig,
    storage: StorageConfig,
    operator: OperatorConfig,
    providers: ProvidersConfig,
    auth: AuthConfig,
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

    /// Returns durable storage settings.
    #[must_use]
    pub const fn storage(&self) -> &StorageConfig {
        &self.storage
    }

    /// Returns operator loop settings.
    #[must_use]
    pub const fn operator(&self) -> &OperatorConfig {
        &self.operator
    }

    /// Returns model provider connectivity settings.
    #[must_use]
    pub const fn providers(&self) -> &ProvidersConfig {
        &self.providers
    }

    /// Returns API authentication settings.
    #[must_use]
    pub const fn auth(&self) -> &AuthConfig {
        &self.auth
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
        if self.operator.reconcile_interval.get().is_zero() {
            return Err(ConfigValidationError::new(
                "operator.reconcileInterval",
                "must be positive",
            ));
        }
        if self.operator.dispatch_interval.get().is_zero() {
            return Err(ConfigValidationError::new(
                "operator.dispatchInterval",
                "must be positive",
            ));
        }
        if !is_http_url(&self.providers.ollama_base_url) {
            return Err(ConfigValidationError::new(
                "providers.ollamaBaseUrl",
                "must use http or https",
            ));
        }
        if !is_http_url(&self.providers.openai_base_url) {
            return Err(ConfigValidationError::new(
                "providers.openaiBaseUrl",
                "must use http or https",
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
        if let Some(storage) = patch.storage {
            self.storage.apply(storage);
        }
        if let Some(operator) = patch.operator {
            self.operator.apply(operator);
        }
        if let Some(providers) = patch.providers {
            self.providers.apply(providers);
        }
        if let Some(auth) = patch.auth {
            self.auth.apply(auth);
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
            storage: StorageConfig::default(),
            operator: OperatorConfig::default(),
            providers: ProvidersConfig::default(),
            auth: AuthConfig::default(),
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

/// Filesystem location for durable operator state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageConfig {
    data_dir: PathBuf,
}

impl StorageConfig {
    /// Returns the directory holding the SQLite database file.
    #[must_use]
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    fn apply(&mut self, patch: StorageConfigPatch) {
        if let Some(value) = patch.data_dir {
            self.data_dir = value;
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("agentkube-data"),
        }
    }
}

/// Reconcile and dispatch cadence for the operator loops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorConfig {
    reconcile_interval: HumanDuration,
    dispatch_interval: HumanDuration,
}

impl OperatorConfig {
    /// Returns how often desired state is reconciled.
    #[must_use]
    pub const fn reconcile_interval(&self) -> HumanDuration {
        self.reconcile_interval
    }

    /// Returns how often queued tasks are dispatched to workers.
    #[must_use]
    pub const fn dispatch_interval(&self) -> HumanDuration {
        self.dispatch_interval
    }

    fn apply(&mut self, patch: OperatorConfigPatch) {
        if let Some(value) = patch.reconcile_interval {
            self.reconcile_interval = value;
        }
        if let Some(value) = patch.dispatch_interval {
            self.dispatch_interval = value;
        }
    }
}

impl Default for OperatorConfig {
    fn default() -> Self {
        Self {
            reconcile_interval: "15s".parse().expect("default duration is valid"),
            dispatch_interval: "1s".parse().expect("default duration is valid"),
        }
    }
}

/// A secret value that never appears in logs or diagnostics.
///
/// Serialization preserves the real value (configuration round-trips must
/// stay intact); `Debug` and `Display` always render `***`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Creates a secret, rejecting empty values.
    pub fn new(value: impl Into<String>) -> Result<Self, SecretStringError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecretStringError);
        }
        Ok(Self(value))
    }

    /// Returns the secret for intentional use (authentication, API keys).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(***)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("***")
    }
}

impl FromStr for SecretString {
    type Err = SecretStringError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Empty secrets are rejected at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretStringError;

impl fmt::Display for SecretStringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("secret must not be empty")
    }
}

impl Error for SecretStringError {}

/// Model provider connectivity for inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvidersConfig {
    ollama_enabled: bool,
    ollama_base_url: String,
    openai_base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    openai_api_key: Option<SecretString>,
}

impl ProvidersConfig {
    /// Returns whether the local Ollama provider is registered.
    #[must_use]
    pub const fn ollama_enabled(&self) -> bool {
        self.ollama_enabled
    }

    /// Returns the Ollama base URL (for example `http://127.0.0.1:11434`).
    #[must_use]
    pub fn ollama_base_url(&self) -> &str {
        &self.ollama_base_url
    }

    /// Returns the OpenAI-compatible base URL.
    #[must_use]
    pub fn openai_base_url(&self) -> &str {
        &self.openai_base_url
    }

    /// Returns the OpenAI API key when configured (never logged).
    #[must_use]
    pub fn openai_api_key(&self) -> Option<&SecretString> {
        self.openai_api_key.as_ref()
    }

    fn apply(&mut self, patch: ProvidersConfigPatch) {
        if let Some(value) = patch.ollama_enabled {
            self.ollama_enabled = value;
        }
        if let Some(value) = patch.ollama_base_url {
            self.ollama_base_url = value;
        }
        if let Some(value) = patch.openai_base_url {
            self.openai_base_url = value;
        }
        if let Some(value) = patch.openai_api_key {
            self.openai_api_key = Some(value);
        }
    }
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            ollama_enabled: true,
            ollama_base_url: "http://127.0.0.1:11434".to_owned(),
            openai_base_url: "https://api.openai.com/v1".to_owned(),
            openai_api_key: None,
        }
    }
}

/// API authentication settings.
///
/// `None` leaves the API open (development default); `Some` enforces Bearer
/// authentication on versioned endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<SecretString>,
}

impl AuthConfig {
    /// Returns the required Bearer token, if any.
    #[must_use]
    pub fn token(&self) -> Option<&SecretString> {
        self.token.as_ref()
    }

    fn apply(&mut self, patch: AuthConfigPatch) {
        if let Some(value) = patch.token {
            self.token = Some(value);
        }
    }
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
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
    pub(crate) storage: Option<StorageConfigPatch>,
    pub(crate) operator: Option<OperatorConfigPatch>,
    pub(crate) providers: Option<ProvidersConfigPatch>,
    pub(crate) auth: Option<AuthConfigPatch>,
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

    /// Applies durable-storage overrides.
    #[must_use]
    pub fn with_storage(mut self, storage: StorageConfigPatch) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Applies operator-loop overrides.
    #[must_use]
    pub fn with_operator(mut self, operator: OperatorConfigPatch) -> Self {
        self.operator = Some(operator);
        self
    }

    /// Applies provider-connectivity overrides.
    #[must_use]
    pub fn with_providers(mut self, providers: ProvidersConfigPatch) -> Self {
        self.providers = Some(providers);
        self
    }

    /// Applies API-authentication overrides.
    #[must_use]
    pub fn with_auth(mut self, auth: AuthConfigPatch) -> Self {
        self.auth = Some(auth);
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

/// Partial durable-storage configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageConfigPatch {
    pub(crate) data_dir: Option<PathBuf>,
}

impl StorageConfigPatch {
    /// Overrides the database directory.
    #[must_use]
    pub fn with_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }
}

/// Partial operator-loop configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorConfigPatch {
    pub(crate) reconcile_interval: Option<HumanDuration>,
    pub(crate) dispatch_interval: Option<HumanDuration>,
}

impl OperatorConfigPatch {
    /// Overrides the reconcile cadence.
    #[must_use]
    pub const fn with_reconcile_interval(mut self, interval: HumanDuration) -> Self {
        self.reconcile_interval = Some(interval);
        self
    }

    /// Overrides the dispatch cadence.
    #[must_use]
    pub const fn with_dispatch_interval(mut self, interval: HumanDuration) -> Self {
        self.dispatch_interval = Some(interval);
        self
    }
}

/// Partial provider-connectivity configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvidersConfigPatch {
    pub(crate) ollama_enabled: Option<bool>,
    pub(crate) ollama_base_url: Option<String>,
    pub(crate) openai_base_url: Option<String>,
    pub(crate) openai_api_key: Option<SecretString>,
}

impl ProvidersConfigPatch {
    /// Enables or disables the local Ollama provider.
    #[must_use]
    pub const fn with_ollama_enabled(mut self, enabled: bool) -> Self {
        self.ollama_enabled = Some(enabled);
        self
    }

    /// Overrides the Ollama base URL.
    #[must_use]
    pub fn with_ollama_base_url(mut self, url: impl Into<String>) -> Self {
        self.ollama_base_url = Some(url.into());
        self
    }

    /// Overrides the OpenAI-compatible base URL.
    #[must_use]
    pub fn with_openai_base_url(mut self, url: impl Into<String>) -> Self {
        self.openai_base_url = Some(url.into());
        self
    }

    /// Sets the OpenAI API key.
    #[must_use]
    pub fn with_openai_api_key(mut self, key: SecretString) -> Self {
        self.openai_api_key = Some(key);
        self
    }
}

/// Partial API-authentication configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthConfigPatch {
    pub(crate) token: Option<SecretString>,
}

impl AuthConfigPatch {
    /// Sets the required Bearer token.
    #[must_use]
    pub fn with_token(mut self, token: SecretString) -> Self {
        self.token = Some(token);
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
