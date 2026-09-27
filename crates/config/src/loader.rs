use crate::{AgentKubeConfig, ConfigError, ConfigSource};

/// Applies ordered configuration sources over safe built-in defaults.
#[derive(Default)]
pub struct ConfigLoader {
    sources: Vec<Box<dyn ConfigSource>>,
}

impl ConfigLoader {
    /// Creates a loader with no external sources.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// Appends a source. Sources added later have higher precedence.
    #[must_use]
    pub fn with_source(mut self, source: impl ConfigSource + 'static) -> Self {
        self.sources.push(Box::new(source));
        self
    }

    /// Loads, merges, and validates the complete configuration.
    pub fn load(&self) -> Result<AgentKubeConfig, ConfigError> {
        let mut config = AgentKubeConfig::default();
        for source in &self.sources {
            config.apply(source.load()?);
        }
        config.validate().map_err(ConfigError::Validation)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvironmentSource, JsonSource, RuntimeEnvironment};

    #[test]
    fn later_sources_override_earlier_sources_at_field_granularity() {
        let file = JsonSource::new(
            "file",
            r#"{
                "environment": "production",
                "worker": { "concurrency": 8, "leaseTimeout": "45s" },
                "telemetry": { "jsonLogs": true }
            }"#,
        );
        let environment = EnvironmentSource::from_iter(
            "AGENTKUBE",
            [
                ("AGENTKUBE_WORKER__CONCURRENCY", "16"),
                ("AGENTKUBE_TELEMETRY__LOG_LEVEL", "debug"),
            ],
        );

        let config = ConfigLoader::new()
            .with_source(file)
            .with_source(environment)
            .load()
            .unwrap();

        assert_eq!(config.environment(), RuntimeEnvironment::Production);
        assert_eq!(config.worker().concurrency().get(), 16);
        assert_eq!(config.worker().lease_timeout().to_string(), "45s");
        assert!(config.telemetry().json_logs());
        assert_eq!(config.telemetry().log_level(), crate::LogLevel::Debug);
    }
}
