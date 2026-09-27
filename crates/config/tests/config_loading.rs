use agentkube_config::{
    ConfigError, ConfigLoader, ConfigPatch, ConfigSource, EnvironmentSource, JsonFileSource,
    LogLevel, RuntimeEnvironment, WorkerConfigPatch,
};
use std::{fs, num::NonZeroU16, path::PathBuf, process, time::SystemTime};

#[test]
fn file_and_environment_sources_form_a_valid_production_configuration() {
    let path = temporary_config_path();
    fs::write(
        &path,
        r#"{
            "environment": "production",
            "controlPlane": {
                "bindAddress": "0.0.0.0:8080",
                "requestTimeout": "1m"
            },
            "worker": {
                "concurrency": 12,
                "heartbeatInterval": "5s",
                "leaseTimeout": "20s"
            },
            "telemetry": {
                "serviceName": "agentkube-control-plane",
                "logLevel": "info",
                "jsonLogs": true,
                "traceSampleRatio": 0.5
            }
        }"#,
    )
    .expect("write temporary configuration");

    let environment = EnvironmentSource::from_iter(
        "AGENTKUBE",
        [
            ("AGENTKUBE_WORKER__CONCURRENCY", "24"),
            ("AGENTKUBE_TELEMETRY__LOG_LEVEL", "warn"),
        ],
    );
    let result = ConfigLoader::new()
        .with_source(JsonFileSource::new(&path))
        .with_source(environment)
        .load();
    fs::remove_file(&path).expect("remove temporary configuration");
    let config = result.expect("load valid layered configuration");

    assert_eq!(config.environment(), RuntimeEnvironment::Production);
    assert_eq!(
        config.control_plane().bind_address().to_string(),
        "0.0.0.0:8080"
    );
    assert_eq!(config.control_plane().request_timeout().to_string(), "1m");
    assert_eq!(config.worker().concurrency().get(), 24);
    assert_eq!(config.telemetry().log_level(), LogLevel::Warn);
    assert!(config.telemetry().json_logs());
    assert_eq!(config.telemetry().trace_sample_ratio().get(), 0.5);
}

#[test]
fn invalid_merged_configuration_is_not_returned() {
    let environment = EnvironmentSource::from_iter(
        "AGENTKUBE",
        [
            ("AGENTKUBE_WORKER__HEARTBEAT_INTERVAL", "30s"),
            ("AGENTKUBE_WORKER__LEASE_TIMEOUT", "10s"),
        ],
    );

    let error = ConfigLoader::new()
        .with_source(environment)
        .load()
        .expect_err("heartbeat cannot outlive its lease");

    assert!(error.to_string().contains("worker.heartbeatInterval"));
}

#[test]
fn custom_sources_can_build_typed_patches_through_the_public_api() {
    struct ProgrammaticSource;

    impl ConfigSource for ProgrammaticSource {
        fn name(&self) -> &str {
            "programmatic"
        }

        fn load(&self) -> Result<ConfigPatch, ConfigError> {
            Ok(ConfigPatch::default()
                .with_environment(RuntimeEnvironment::Test)
                .with_worker(
                    WorkerConfigPatch::default()
                        .with_concurrency(NonZeroU16::new(2).expect("non-zero concurrency")),
                ))
        }
    }

    let config = ConfigLoader::new()
        .with_source(ProgrammaticSource)
        .load()
        .expect("load programmatic source");

    assert_eq!(config.environment(), RuntimeEnvironment::Test);
    assert_eq!(config.worker().concurrency().get(), 2);
}

fn temporary_config_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time after Unix epoch")
        .as_nanos();
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data");
    fs::create_dir_all(&directory).expect("create test data directory");
    directory.join(format!("agentkube-config-{}-{nonce}.json", process::id()))
}
