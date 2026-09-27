//! Shared HTTP plumbing for provider adapters.
//!
//! Cost estimates follow the same convention as the scripted provider:
//! input tokens are estimated from request bytes and priced with catalog
//! list prices using ceiling division. Nothing is synthesized: usage always
//! reflects provider-reported counts.

use agentkube_agents::{ModelName, ProviderName};
use agentkube_providers::{
    CostEstimate, GenerationRequest, MessageRole, ModelCapabilities, ProviderCapabilities,
    ProviderError, ProviderErrorKind, ProviderResult,
};
use std::{error::Error, fmt, time::Duration};

/// Default request deadline for local Ollama generations.
pub(crate) const OLLAMA_TIMEOUT: Duration = Duration::from_secs(300);
/// Default request deadline for hosted OpenAI generations.
pub(crate) const OPENAI_TIMEOUT: Duration = Duration::from_secs(120);
/// Largest error body kept for diagnostics.
pub(crate) const ERROR_PREVIEW_LIMIT: usize = 300;

/// Failure to construct a provider adapter (never a per-request failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// The base URL is missing, has a non-HTTP(S) scheme, or is malformed.
    InvalidBaseUrl(String),
    /// The API key is missing or empty.
    EmptyApiKey,
    /// The model catalog is empty or names a model twice.
    InvalidCatalog(String),
    /// The HTTP client could not be constructed.
    Transport(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl(value) => {
                write!(formatter, "invalid provider base URL {value:?}")
            }
            Self::EmptyApiKey => formatter.write_str("API key must not be empty"),
            Self::InvalidCatalog(message) => {
                write!(formatter, "invalid model catalog: {message}")
            }
            Self::Transport(message) => {
                write!(formatter, "cannot build HTTP client: {message}")
            }
        }
    }
}

impl Error for AdapterError {}

/// Validates an HTTP(S) base URL and strips trailing slashes.
pub(crate) fn normalize_base_url(raw: &str) -> Result<String, AdapterError> {
    let trimmed = raw.trim();
    let parsed = reqwest::Url::parse(trimmed)
        .map_err(|_| AdapterError::InvalidBaseUrl(trimmed.to_owned()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(AdapterError::InvalidBaseUrl(trimmed.to_owned())),
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(AdapterError::InvalidBaseUrl(trimmed.to_owned()));
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// Builds a client that never logs credentials and always bounds requests.
pub(crate) fn build_client(timeout: Duration) -> Result<reqwest::Client, AdapterError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(format!("agentkube/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| AdapterError::Transport(error.to_string()))
}

/// Maps a portable role to its wire name on both supported APIs.
pub(crate) const fn role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// Converts advertised tools to OpenAI-style function definitions.
///
/// Both Ollama and OpenAI-compatible endpoints accept this shape, so tools
/// are passed through rather than silently dropped.
pub(crate) fn tool_definitions(request: &GenerationRequest) -> Vec<serde_json::Value> {
    request
        .tools()
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name().as_str(),
                    "description": tool.description(),
                    "parameters": tool.input_schema(),
                }
            })
        })
        .collect()
}

/// Estimates maximum cost with catalog list prices (ceiling division).
pub(crate) fn estimate_cost(
    provider: &ProviderName,
    capabilities: &ProviderCapabilities,
    request: &GenerationRequest,
) -> ProviderResult<CostEstimate> {
    let model = capabilities.model(request.model()).ok_or_else(|| {
        ProviderError::new(
            provider.clone(),
            ProviderErrorKind::ModelNotFound,
            format!("model {} is not in the provider catalog", request.model()),
        )
    })?;
    let input = priced_tokens(estimated_input_tokens(request), model.input_price())
        .ok_or_else(|| overflow(provider))?;
    let output = priced_tokens(
        u64::from(request.max_output_tokens().get()),
        model.output_price(),
    )
    .ok_or_else(|| overflow(provider))?;
    Ok(CostEstimate::new(input, output))
}

fn overflow(provider: &ProviderName) -> ProviderError {
    ProviderError::new(
        provider.clone(),
        ProviderErrorKind::Internal,
        "cost estimate exceeded the supported range",
    )
}

/// Estimates input tokens from request bytes (four bytes per token).
fn estimated_input_tokens(request: &GenerationRequest) -> u64 {
    request.messages().iter().fold(0u64, |total, message| {
        let content = message
            .content()
            .map_or(0, |content| content.as_str().len() as u64);
        total.saturating_add(content.div_ceil(4))
    })
}

fn priced_tokens(tokens: u64, price_per_million: u64) -> Option<u64> {
    let numerator = u128::from(tokens)
        .checked_mul(u128::from(price_per_million))?
        .checked_add(999_999)?;
    u64::try_from(numerator / 1_000_000).ok()
}

/// Builds a catalog or reports why it cannot be used.
pub(crate) fn build_catalog(
    models: Vec<(ModelName, ModelCapabilities)>,
) -> Result<ProviderCapabilities, AdapterError> {
    ProviderCapabilities::new(models)
        .map_err(|error| AdapterError::InvalidCatalog(error.to_string()))
}

/// Maps transport failures to normalized, retry-aware errors.
pub(crate) fn send_error(provider: &ProviderName, error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        return ProviderError::new(
            provider.clone(),
            ProviderErrorKind::Timeout,
            format!("request timed out: {error}"),
        );
    }
    if error.is_connect() {
        return ProviderError::new(
            provider.clone(),
            ProviderErrorKind::Unavailable,
            format!("cannot connect: {error}"),
        );
    }
    ProviderError::new(
        provider.clone(),
        ProviderErrorKind::Internal,
        format!("request failed: {error}"),
    )
}

/// Maps HTTP error statuses to normalized errors with a truncated preview.
pub(crate) fn status_error(
    provider: &ProviderName,
    status: reqwest::StatusCode,
    preview: &str,
) -> ProviderError {
    let preview: String = preview.chars().take(ERROR_PREVIEW_LIMIT).collect();
    let message = if preview.trim().is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        format!("HTTP {}: {preview}", status.as_u16())
    };
    let kind = match status.as_u16() {
        401 | 403 => ProviderErrorKind::Authentication,
        404 => ProviderErrorKind::ModelNotFound,
        429 => ProviderErrorKind::RateLimited { retry_after: None },
        400 | 422 => ProviderErrorKind::InvalidRequest,
        500..=599 => ProviderErrorKind::Unavailable,
        _ => ProviderErrorKind::Protocol,
    };
    ProviderError::new(provider.clone(), kind, message)
}

/// Reads a bounded error preview without ever logging credentials.
pub(crate) async fn error_preview(response: reqwest::Response) -> (reqwest::StatusCode, String) {
    let status = response.status();
    let preview = response
        .bytes()
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    (status, preview)
}
