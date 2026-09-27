//! Deterministic configuration resolution for `akctl`.
//!
//! Resolution order for the API URL is `--server`, then `AGENTKUBE_SERVER`,
//! then `http://127.0.0.1:8080`. Timeouts default to 30 seconds.

use crate::{args::OutputMode, error::CliError};
use std::time::Duration;

/// Default API base URL when no flag or environment override is present.
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8080";
/// Default complete-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Environment variable overriding the API base URL.
pub const SERVER_ENV_VAR: &str = "AGENTKUBE_SERVER";
/// Environment variable carrying an optional bearer token.
pub const TOKEN_ENV_VAR: &str = "AGENTKUBE_TOKEN";

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Normalized base URL without trailing slashes.
    pub server_url: String,
    /// Complete operation deadline applied to each command.
    pub timeout: Duration,
    /// Requested output mode, or `None` when the flag was omitted.
    pub output: Option<OutputMode>,
    /// Whether diagnostic color is disabled.
    pub no_color: bool,
    /// Verbosity level (repeatable `-v`).
    pub verbose: u8,
    /// Optional bearer token from the environment (never logged).
    pub token: Option<String>,
}

impl ResolvedConfig {
    /// Returns the effective output mode for list/get style commands.
    #[must_use]
    pub const fn effective_output(&self, describe_default: OutputMode) -> OutputMode {
        match self.output {
            Some(mode) => mode,
            None => describe_default,
        }
    }
}

/// Builds configuration from parsed CLI flags and the process environment.
pub fn build_config(
    server_flag: Option<&str>,
    timeout_raw: &str,
    output: Option<OutputMode>,
    no_color: bool,
    verbose: u8,
) -> Result<ResolvedConfig, CliError> {
    Ok(ResolvedConfig {
        server_url: resolve_server_url(server_flag)?,
        timeout: parse_timeout(timeout_raw)?,
        output,
        no_color,
        verbose,
        token: resolve_token(),
    })
}

/// Resolves the API base URL using flag, environment, then default.
pub fn resolve_server_url(flag: Option<&str>) -> Result<String, CliError> {
    if let Some(raw) = flag {
        return normalize_server_url(raw);
    }
    let from_env = std::env::var(SERVER_ENV_VAR)
        .ok()
        .filter(|raw| !raw.trim().is_empty());
    if let Some(raw) = from_env {
        return normalize_server_url(&raw);
    }
    normalize_server_url(DEFAULT_SERVER_URL)
}

/// Pure helper for precedence testing without touching the process environment.
#[cfg(test)]
fn resolve_server_url_with_override(
    flag: Option<&str>,
    env_value: Option<&str>,
) -> Result<String, CliError> {
    if let Some(raw) = flag {
        return normalize_server_url(raw);
    }
    if let Some(raw) = env_value.filter(|raw| !raw.trim().is_empty()) {
        return normalize_server_url(raw);
    }
    normalize_server_url(DEFAULT_SERVER_URL)
}

/// Normalizes a base URL: validates scheme, rejects credentials, query,
/// fragments, and malformed values, then strips trailing `/` characters.
pub fn normalize_server_url(raw: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage("server URL must not be empty"));
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|error| CliError::usage(format!("invalid server URL {trimmed:?}: {error}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CliError::usage(format!(
                "invalid server URL {trimmed:?}: scheme {other:?} must be http or https"
            )));
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CliError::usage(format!(
            "invalid server URL {trimmed:?}: credentials must not be embedded in the URL"
        )));
    }
    if parsed.query().is_some() {
        return Err(CliError::usage(format!(
            "invalid server URL {trimmed:?}: query strings are not allowed"
        )));
    }
    if parsed.fragment().is_some() {
        return Err(CliError::usage(format!(
            "invalid server URL {trimmed:?}: fragments are not allowed"
        )));
    }
    if parsed.host_str().is_none() {
        return Err(CliError::usage(format!(
            "invalid server URL {trimmed:?}: missing host"
        )));
    }
    let normalized = trimmed.trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(CliError::usage("server URL must not be empty"));
    }
    Ok(normalized)
}

/// Parses a human duration such as `30s`, `500ms`, or `1m`.
pub fn parse_timeout(raw: &str) -> Result<Duration, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage("timeout must not be empty"));
    }
    if let Ok(duration) = humantime::parse_duration(trimmed) {
        if duration.is_zero() {
            return Err(CliError::usage("timeout must be greater than zero"));
        }
        return Ok(duration);
    }
    // Accept bare seconds for convenience (for example `--timeout 30`).
    if let Ok(seconds) = trimmed.parse::<u64>() {
        if seconds == 0 {
            return Err(CliError::usage("timeout must be greater than zero"));
        }
        return Ok(Duration::from_secs(seconds));
    }
    Err(CliError::usage(format!(
        "invalid timeout {trimmed:?}: expected a duration like 30s, 500ms, or 1m"
    )))
}

/// Reads the optional bearer token without ever logging its value.
fn resolve_token() -> Option<String> {
    std::env::var(TOKEN_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_server_is_used_without_overrides() {
        assert_eq!(
            resolve_server_url_with_override(None, None).unwrap(),
            DEFAULT_SERVER_URL
        );
    }

    #[test]
    fn flag_takes_precedence_over_environment() {
        let resolved =
            resolve_server_url_with_override(Some("http://flag:8080"), Some("http://env:8080"))
                .unwrap();

        assert_eq!(resolved, "http://flag:8080");
    }

    #[test]
    fn environment_overrides_default() {
        let resolved = resolve_server_url_with_override(None, Some("http://env:8080")).unwrap();

        assert_eq!(resolved, "http://env:8080");
    }

    #[test]
    fn trailing_slashes_are_stripped() {
        assert_eq!(
            normalize_server_url("http://127.0.0.1:8080///").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn non_http_schemes_queries_fragments_and_credentials_are_rejected() {
        for raw in [
            "ftp://example.com",
            "http://example.com/api?x=1",
            "https://example.com/#frag",
            "http://user:pass@example.com",
            "not a url",
            "",
        ] {
            assert!(
                normalize_server_url(raw).is_err(),
                "{raw:?} should be rejected"
            );
        }
    }

    #[test]
    fn timeout_parses_human_durations_and_bare_seconds() {
        assert_eq!(parse_timeout("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_timeout("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_timeout("30").unwrap(), Duration::from_secs(30));
        assert!(parse_timeout("0s").is_err());
        assert!(parse_timeout("banana").is_err());
    }
}
