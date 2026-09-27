use crate::{
    ConfigError, ConfigPatch, ControlPlaneConfigPatch, LogLevel, RuntimeEnvironment, SamplingRatio,
    TelemetryConfigPatch, WorkerConfigPatch,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Produces a partial configuration for ordered merging.
pub trait ConfigSource: Send + Sync {
    /// Returns a diagnostic name for this source.
    fn name(&self) -> &str;

    /// Reads and decodes a partial configuration.
    fn load(&self) -> Result<ConfigPatch, ConfigError>;
}

/// JSON configuration embedded in memory.
#[derive(Debug, Clone)]
pub struct JsonSource {
    name: String,
    document: String,
}

impl JsonSource {
    /// Creates an in-memory JSON source.
    #[must_use]
    pub fn new(name: impl Into<String>, document: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            document: document.into(),
        }
    }
}

impl ConfigSource for JsonSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn load(&self) -> Result<ConfigPatch, ConfigError> {
        decode_json(self.name(), &self.document)
    }
}

/// Partial JSON configuration loaded from a filesystem path.
#[derive(Debug, Clone)]
pub struct JsonFileSource {
    path: PathBuf,
}

impl JsonFileSource {
    /// Creates a file source without reading it immediately.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the configured file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ConfigSource for JsonFileSource {
    fn name(&self) -> &str {
        self.path
            .to_str()
            .unwrap_or("<non-UTF-8 configuration path>")
    }

    fn load(&self) -> Result<ConfigPatch, ConfigError> {
        let document = fs::read_to_string(&self.path).map_err(|source| ConfigError::Io {
            source_name: self.name().to_owned(),
            path: self.path.clone(),
            source,
        })?;
        decode_json(self.name(), &document)
    }
}

/// Explicitly supported environment variables collected under one prefix.
#[derive(Debug, Clone)]
pub struct EnvironmentSource {
    name: String,
    prefix: String,
    values: BTreeMap<String, String>,
}

impl EnvironmentSource {
    /// Captures matching variables from the current process environment.
    #[must_use]
    pub fn from_current_process(prefix: impl Into<String>) -> Self {
        Self::from_iter(prefix, std::env::vars())
    }

    /// Creates a deterministic environment source from key-value pairs.
    #[must_use]
    pub fn from_iter<K, V, I>(prefix: impl Into<String>, values: I) -> Self
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        let prefix = prefix.into().trim_end_matches('_').to_ascii_uppercase();
        let marker = format!("{prefix}_");
        let values = values
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .filter(|(key, _)| key.starts_with(&marker))
            .collect();

        Self {
            name: format!("environment:{prefix}"),
            prefix,
            values,
        }
    }

    /// Returns the normalized environment prefix.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

impl ConfigSource for EnvironmentSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn load(&self) -> Result<ConfigPatch, ConfigError> {
        let mut patch = ConfigPatch::default();
        let marker = format!("{}_", self.prefix);

        for (full_key, value) in &self.values {
            let key = full_key
                .strip_prefix(&marker)
                .expect("values are filtered by prefix");
            match key {
                "ENVIRONMENT" => {
                    patch.environment = Some(parse_environment(self, full_key, value)?)
                }
                "SHUTDOWN_GRACE_PERIOD" => {
                    patch.shutdown_grace_period = Some(parse(self, full_key, value)?)
                }
                "CONTROL_PLANE__BIND_ADDRESS" => {
                    control_plane_patch(&mut patch).bind_address =
                        Some(parse(self, full_key, value)?)
                }
                "CONTROL_PLANE__REQUEST_TIMEOUT" => {
                    control_plane_patch(&mut patch).request_timeout =
                        Some(parse(self, full_key, value)?)
                }
                "CONTROL_PLANE__MAX_REQUEST_BODY_BYTES" => {
                    control_plane_patch(&mut patch).max_request_body_bytes =
                        Some(parse(self, full_key, value)?)
                }
                "WORKER__CONCURRENCY" => {
                    worker_patch(&mut patch).concurrency = Some(parse(self, full_key, value)?)
                }
                "WORKER__HEARTBEAT_INTERVAL" => {
                    worker_patch(&mut patch).heartbeat_interval =
                        Some(parse(self, full_key, value)?)
                }
                "WORKER__LEASE_TIMEOUT" => {
                    worker_patch(&mut patch).lease_timeout = Some(parse(self, full_key, value)?)
                }
                "TELEMETRY__SERVICE_NAME" => {
                    telemetry_patch(&mut patch).service_name = Some(value.clone())
                }
                "TELEMETRY__LOG_LEVEL" => {
                    telemetry_patch(&mut patch).log_level =
                        Some(parse_log_level(self, full_key, value)?)
                }
                "TELEMETRY__JSON_LOGS" => {
                    telemetry_patch(&mut patch).json_logs = Some(parse_bool(self, full_key, value)?)
                }
                "TELEMETRY__TRACE_SAMPLE_RATIO" => {
                    telemetry_patch(&mut patch).trace_sample_ratio =
                        Some(parse_sampling_ratio(self, full_key, value)?)
                }
                _ => {
                    return Err(ConfigError::invalid_value(
                        self.name(),
                        full_key,
                        value,
                        "unknown AgentKube configuration key",
                    ));
                }
            }
        }
        Ok(patch)
    }
}

fn decode_json(source_name: &str, document: &str) -> Result<ConfigPatch, ConfigError> {
    serde_json::from_str(document).map_err(|source| ConfigError::Decode {
        source_name: source_name.to_owned(),
        source,
    })
}

fn control_plane_patch(patch: &mut ConfigPatch) -> &mut ControlPlaneConfigPatch {
    patch
        .control_plane
        .get_or_insert_with(ControlPlaneConfigPatch::default)
}

fn worker_patch(patch: &mut ConfigPatch) -> &mut WorkerConfigPatch {
    patch.worker.get_or_insert_with(WorkerConfigPatch::default)
}

fn telemetry_patch(patch: &mut ConfigPatch) -> &mut TelemetryConfigPatch {
    patch
        .telemetry
        .get_or_insert_with(TelemetryConfigPatch::default)
}

fn parse<T>(source: &EnvironmentSource, key: &str, value: &str) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| ConfigError::invalid_value(source.name(), key, value, error.to_string()))
}

fn parse_environment(
    source: &EnvironmentSource,
    key: &str,
    value: &str,
) -> Result<RuntimeEnvironment, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "development" => Ok(RuntimeEnvironment::Development),
        "test" => Ok(RuntimeEnvironment::Test),
        "production" => Ok(RuntimeEnvironment::Production),
        _ => Err(ConfigError::invalid_value(
            source.name(),
            key,
            value,
            "expected development, test, or production",
        )),
    }
}

fn parse_log_level(
    source: &EnvironmentSource,
    key: &str,
    value: &str,
) -> Result<LogLevel, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "trace" => Ok(LogLevel::Trace),
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warn" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(ConfigError::invalid_value(
            source.name(),
            key,
            value,
            "expected trace, debug, info, warn, or error",
        )),
    }
}

fn parse_bool(source: &EnvironmentSource, key: &str, value: &str) -> Result<bool, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::invalid_value(
            source.name(),
            key,
            value,
            "expected a boolean",
        )),
    }
}

fn parse_sampling_ratio(
    source: &EnvironmentSource,
    key: &str,
    value: &str,
) -> Result<SamplingRatio, ConfigError> {
    let parsed: f64 = parse(source, key, value)?;
    SamplingRatio::new(parsed)
        .map_err(|error| ConfigError::invalid_value(source.name(), key, value, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_source_decodes_all_supported_value_categories() {
        let source = EnvironmentSource::from_iter(
            "agentkube",
            [
                ("AGENTKUBE_ENVIRONMENT", "production"),
                ("AGENTKUBE_CONTROL_PLANE__BIND_ADDRESS", "0.0.0.0:9000"),
                ("AGENTKUBE_WORKER__CONCURRENCY", "32"),
                ("AGENTKUBE_TELEMETRY__JSON_LOGS", "yes"),
                ("AGENTKUBE_TELEMETRY__TRACE_SAMPLE_RATIO", "0.25"),
            ],
        );

        let patch = source.load().unwrap();

        assert_eq!(patch.environment, Some(RuntimeEnvironment::Production));
        assert_eq!(
            patch.control_plane.unwrap().bind_address.unwrap().port(),
            9000
        );
        assert_eq!(patch.worker.unwrap().concurrency.unwrap().get(), 32);
        let telemetry = patch.telemetry.unwrap();
        assert_eq!(telemetry.json_logs, Some(true));
        assert_eq!(telemetry.trace_sample_ratio.unwrap().get(), 0.25);
    }

    #[test]
    fn unknown_prefixed_environment_key_is_rejected() {
        let source =
            EnvironmentSource::from_iter("AGENTKUBE", [("AGENTKUBE_WORKER__TYPO", "value")]);

        assert!(matches!(
            source.load(),
            Err(ConfigError::InvalidValue { .. })
        ));
    }

    #[test]
    fn json_rejects_unknown_fields() {
        let source = JsonSource::new("test", r#"{"worker":{"concorrency":4}}"#);

        assert!(matches!(source.load(), Err(ConfigError::Decode { .. })));
    }
}
