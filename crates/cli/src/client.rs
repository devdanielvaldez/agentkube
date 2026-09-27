//! Typed HTTP client for `agentkube-api` with safe retry policy.
//!
//! Only idempotent `GET` requests are retried, and only on connection resets
//! or `502`, `503`, and `504`. Mutating requests are never retried because the
//! API provides no idempotency keys.

use crate::error::CliError;
use agentkube_agents::{AgentDeploymentDocument, AgentDocument};
use agentkube_protocol::{ApiError, ListResponse};
use agentkube_tasks::TaskDocument;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

/// Maximum buffered response body (2 MiB, matching the API ceiling).
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Preview length used when a non-`ApiError` body must be summarized.
const ERROR_PREVIEW_LIMIT: usize = 500;

/// Liveness response returned by `/healthz` and `/readyz`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct HealthResponse {
    /// Process status, normally `ok`.
    pub status: String,
    /// Server version.
    pub version: String,
}

/// Typed API client.
#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: String,
    inner: reqwest::Client,
    timeout: Duration,
    token: Option<String>,
    verbose: u8,
}

impl ApiClient {
    /// Creates a client with Rustls TLS and a descriptive user agent.
    pub fn new(
        base_url: String,
        timeout: Duration,
        token: Option<String>,
        verbose: u8,
    ) -> Result<Self, CliError> {
        let user_agent = format!("akctl/{}", env!("CARGO_PKG_VERSION"));
        let inner = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(user_agent)
            .build()
            .map_err(|error| {
                CliError::transport("build HTTP client", format!("cannot build client: {error}"))
            })?;
        Ok(Self {
            base_url,
            inner,
            timeout,
            token,
            verbose,
        })
    }

    /// Returns the normalized base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn bearer(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    /// Sends one `GET` with bounded retries. All other methods send once.
    async fn send_get(&self, operation: &str, url: String) -> Result<reqwest::Response, CliError> {
        let deadline = Instant::now() + self.timeout;
        let mut attempt: u32 = 0;
        let mut backoff = Duration::from_millis(100);
        loop {
            let request = self
                .bearer(self.inner.get(&url))
                .header("Accept", "application/json");
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if is_retryable_status(status) {
                        let status_u16 = status.as_u16();
                        let bytes = match response.bytes().await {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                return Err(CliError::transport(
                                    operation,
                                    format!("cannot read error response: {error}"),
                                ));
                            }
                        };
                        let retry_after = parse_retry_after_from_bytes(&bytes);
                        if Instant::now() >= deadline {
                            return Err(error_from_bytes(operation, status_u16, &bytes));
                        }
                        let delay = retry_after.unwrap_or(backoff);
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Err(error_from_bytes(operation, status_u16, &bytes));
                        }
                        let sleep_for = delay.min(remaining).min(Duration::from_secs(2));
                        self.log_retry(operation, attempt, &format!("HTTP {status}"), sleep_for);
                        tokio::time::sleep(sleep_for).await;
                        attempt = attempt.saturating_add(1);
                        backoff = (backoff * 2).min(Duration::from_secs(1));
                        continue;
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if !is_retryable_error(&error) || Instant::now() >= deadline {
                        return Err(CliError::transport(
                            operation,
                            format!("request failed: {error}"),
                        ));
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(CliError::transport(
                            operation,
                            format!("request failed: {error}"),
                        ));
                    }
                    let sleep_for = backoff.min(remaining).min(Duration::from_secs(2));
                    self.log_retry(operation, attempt, &error.to_string(), sleep_for);
                    tokio::time::sleep(sleep_for).await;
                    attempt = attempt.saturating_add(1);
                    backoff = (backoff * 2).min(Duration::from_secs(1));
                }
            }
        }
    }

    /// Sends a mutating request exactly once (never retried).
    /// Fallible wrapper for mutating requests without retries.
    async fn send_once_result(
        &self,
        method: Method,
        operation: &str,
        url: String,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, CliError> {
        let mut builder = self.inner.request(method, &url);
        builder = self.bearer(builder);
        builder = builder.header("Accept", "application/json");
        if let Some(payload) = body {
            builder = builder
                .header("Content-Type", "application/json")
                .json(&payload);
        }
        builder
            .send()
            .await
            .map_err(|error| CliError::transport(operation, format!("request failed: {error}")))
    }

    #[allow(dead_code)]
    fn log_retry(&self, operation: &str, attempt: u32, cause: &str, delay: Duration) {
        if self.verbose > 0 {
            eprintln!(
                "retrying {operation} (attempt {}): {cause}; waiting {}ms",
                attempt + 1,
                delay.as_millis()
            );
        }
    }

    /// Reads a bounded JSON body for successful responses.
    async fn decode_success<T: DeserializeOwned>(
        operation: &str,
        response: reqwest::Response,
    ) -> Result<T, CliError> {
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|error| {
            CliError::transport(operation, format!("cannot read response: {error}"))
        })?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(CliError::transport(
                operation,
                format!(
                    "response body exceeds the {MAX_RESPONSE_BYTES} byte ceiling and was discarded"
                ),
            ));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            CliError::transport(
                operation,
                format!("invalid success response (HTTP {status}): {error}"),
            )
        })
    }

    /// Converts any HTTP response into success value or typed error.
    async fn handle<T: DeserializeOwned>(
        operation: &str,
        response: reqwest::Response,
    ) -> Result<T, CliError> {
        if response.status().is_success() {
            return Self::decode_success(operation, response).await;
        }
        Err(response_to_error(operation, response).await)
    }

    /// Converts 204/404-aware responses for delete operations.
    async fn handle_empty(operation: &str, response: reqwest::Response) -> Result<(), CliError> {
        if response.status().is_success() {
            // Drain bounded body without logging it.
            let bytes = response.bytes().await.map_err(|error| {
                CliError::transport(operation, format!("cannot read response: {error}"))
            })?;
            if bytes.len() > MAX_RESPONSE_BYTES {
                return Err(CliError::transport(
                    operation,
                    format!(
                        "response body exceeds the {MAX_RESPONSE_BYTES} byte ceiling and was discarded"
                    ),
                ));
            }
            return Ok(());
        }
        Err(response_to_error(operation, response).await)
    }

    /// Checks liveness.
    pub async fn health(&self) -> Result<HealthResponse, CliError> {
        let url = self.url("/healthz");
        let response = self.send_get("check health", url).await?;
        Self::handle("check health", response).await
    }

    /// Checks readiness.
    pub async fn ready(&self) -> Result<HealthResponse, CliError> {
        let url = self.url("/readyz");
        let response = self.send_get("check readiness", url).await?;
        Self::handle("check readiness", response).await
    }

    /// Fetches one agent by name.
    pub async fn get_agent(&self, name: &str) -> Result<AgentDocument, CliError> {
        let operation = format!("get agent {name:?}");
        let url = self.url(&format!("/v1/agents/{name}"));
        let response = self.send_get(&operation, url).await?;
        Self::handle(&operation, response).await
    }

    /// Fetches one agent list page.
    pub async fn list_agents_page(
        &self,
        limit: Option<u32>,
        continue_token: Option<&str>,
    ) -> Result<ListResponse<AgentDocument>, CliError> {
        let operation = "list agents".to_owned();
        let mut url = self.url("/v1/agents");
        append_pagination(&mut url, limit, continue_token);
        let response = self.send_get(&operation, url).await?;
        Self::handle(&operation, response).await
    }

    /// Follows continuation tokens and combines every page.
    pub async fn list_all_agents(
        &self,
        page_size: Option<u32>,
        start: Option<&str>,
    ) -> Result<Vec<AgentDocument>, CliError> {
        let mut items = Vec::new();
        let mut next: Option<String> = start.map(ToOwned::to_owned);
        let mut seen: HashSet<String> = HashSet::new();
        loop {
            let page = self.list_agents_page(page_size, next.as_deref()).await?;
            let token = page_token(&page);
            items.extend(page.into_items());
            match token {
                None => break,
                Some(value) => {
                    if !seen.insert(value.clone()) {
                        return Err(CliError::transport(
                            "list agents",
                            "server returned a repeated continuation token",
                        ));
                    }
                    next = Some(value);
                }
            }
        }
        Ok(items)
    }

    /// Creates an agent.
    pub async fn create_agent(&self, document: &AgentDocument) -> Result<AgentDocument, CliError> {
        let operation = format!("create agent {:?}", document.metadata().name().as_str());
        let body = serde_json::to_value(document).map_err(|error| {
            CliError::invalid_input(format!("cannot encode agent document: {error}"))
        })?;
        let response = self
            .send_once_result(Method::POST, &operation, self.url("/v1/agents"), Some(body))
            .await?;
        Self::handle(&operation, response).await
    }

    /// Replaces an agent.
    pub async fn replace_agent(
        &self,
        name: &str,
        document: &AgentDocument,
    ) -> Result<AgentDocument, CliError> {
        let operation = format!("replace agent {name:?}");
        let body = serde_json::to_value(document).map_err(|error| {
            CliError::invalid_input(format!("cannot encode agent document: {error}"))
        })?;
        let response = self
            .send_once_result(
                Method::PUT,
                &operation,
                self.url(&format!("/v1/agents/{name}")),
                Some(body),
            )
            .await?;
        Self::handle(&operation, response).await
    }

    /// Deletes an agent.
    pub async fn delete_agent(&self, name: &str) -> Result<(), CliError> {
        let operation = format!("delete agent {name:?}");
        let response = self
            .send_once_result(
                Method::DELETE,
                &operation,
                self.url(&format!("/v1/agents/{name}")),
                None,
            )
            .await?;
        Self::handle_empty(&operation, response).await
    }

    /// Fetches one deployment by name.
    pub async fn get_deployment(&self, name: &str) -> Result<AgentDeploymentDocument, CliError> {
        let operation = format!("get deployment {name:?}");
        let url = self.url(&format!("/v1/deployments/{name}"));
        let response = self.send_get(&operation, url).await?;
        Self::handle(&operation, response).await
    }

    /// Fetches one deployment list page.
    pub async fn list_deployments_page(
        &self,
        limit: Option<u32>,
        continue_token: Option<&str>,
    ) -> Result<ListResponse<AgentDeploymentDocument>, CliError> {
        let operation = "list deployments".to_owned();
        let mut url = self.url("/v1/deployments");
        append_pagination(&mut url, limit, continue_token);
        let response = self.send_get(&operation, url).await?;
        Self::handle(&operation, response).await
    }

    /// Follows continuation tokens for deployments.
    pub async fn list_all_deployments(
        &self,
        page_size: Option<u32>,
        start: Option<&str>,
    ) -> Result<Vec<AgentDeploymentDocument>, CliError> {
        let mut items = Vec::new();
        let mut next: Option<String> = start.map(ToOwned::to_owned);
        let mut seen: HashSet<String> = HashSet::new();
        loop {
            let page = self
                .list_deployments_page(page_size, next.as_deref())
                .await?;
            let token = page_token(&page);
            items.extend(page.into_items());
            match token {
                None => break,
                Some(value) => {
                    if !seen.insert(value.clone()) {
                        return Err(CliError::transport(
                            "list deployments",
                            "server returned a repeated continuation token",
                        ));
                    }
                    next = Some(value);
                }
            }
        }
        Ok(items)
    }

    /// Creates a deployment.
    pub async fn create_deployment(
        &self,
        document: &AgentDeploymentDocument,
    ) -> Result<AgentDeploymentDocument, CliError> {
        let operation = format!(
            "create deployment {:?}",
            document.metadata().name().as_str()
        );
        let body = serde_json::to_value(document).map_err(|error| {
            CliError::invalid_input(format!("cannot encode deployment document: {error}"))
        })?;
        let response = self
            .send_once_result(
                Method::POST,
                &operation,
                self.url("/v1/deployments"),
                Some(body),
            )
            .await?;
        Self::handle(&operation, response).await
    }

    /// Replaces a deployment.
    pub async fn replace_deployment(
        &self,
        name: &str,
        document: &AgentDeploymentDocument,
    ) -> Result<AgentDeploymentDocument, CliError> {
        let operation = format!("replace deployment {name:?}");
        let body = serde_json::to_value(document).map_err(|error| {
            CliError::invalid_input(format!("cannot encode deployment document: {error}"))
        })?;
        let response = self
            .send_once_result(
                Method::PUT,
                &operation,
                self.url(&format!("/v1/deployments/{name}")),
                Some(body),
            )
            .await?;
        Self::handle(&operation, response).await
    }

    /// Deletes a deployment.
    pub async fn delete_deployment(&self, name: &str) -> Result<(), CliError> {
        let operation = format!("delete deployment {name:?}");
        let response = self
            .send_once_result(
                Method::DELETE,
                &operation,
                self.url(&format!("/v1/deployments/{name}")),
                None,
            )
            .await?;
        Self::handle_empty(&operation, response).await
    }

    /// Fetches one task by name.
    pub async fn get_task(&self, name: &str) -> Result<TaskDocument, CliError> {
        let operation = format!("get task {name:?}");
        let url = self.url(&format!("/v1/tasks/{name}"));
        let response = self.send_get(&operation, url).await?;
        Self::handle(&operation, response).await
    }

    /// Fetches one task list page.
    pub async fn list_tasks_page(
        &self,
        limit: Option<u32>,
        continue_token: Option<&str>,
    ) -> Result<ListResponse<TaskDocument>, CliError> {
        let operation = "list tasks".to_owned();
        let mut url = self.url("/v1/tasks");
        append_pagination(&mut url, limit, continue_token);
        let response = self.send_get(&operation, url).await?;
        Self::handle(&operation, response).await
    }

    /// Follows continuation tokens for tasks.
    pub async fn list_all_tasks(
        &self,
        page_size: Option<u32>,
        start: Option<&str>,
    ) -> Result<Vec<TaskDocument>, CliError> {
        let mut items = Vec::new();
        let mut next: Option<String> = start.map(ToOwned::to_owned);
        let mut seen: HashSet<String> = HashSet::new();
        loop {
            let page = self.list_tasks_page(page_size, next.as_deref()).await?;
            let token = page_token(&page);
            items.extend(page.into_items());
            match token {
                None => break,
                Some(value) => {
                    if !seen.insert(value.clone()) {
                        return Err(CliError::transport(
                            "list tasks",
                            "server returned a repeated continuation token",
                        ));
                    }
                    next = Some(value);
                }
            }
        }
        Ok(items)
    }

    /// Creates a task (create-only; tasks are execution records).
    pub async fn create_task(&self, document: &TaskDocument) -> Result<TaskDocument, CliError> {
        let operation = format!("create task {:?}", document.metadata().name().as_str());
        let body = serde_json::to_value(document).map_err(|error| {
            CliError::invalid_input(format!("cannot encode task document: {error}"))
        })?;
        let response = self
            .send_once_result(Method::POST, &operation, self.url("/v1/tasks"), Some(body))
            .await?;
        Self::handle(&operation, response).await
    }

    /// Deletes a task (only terminal tasks are accepted by the server).
    pub async fn delete_task(&self, name: &str) -> Result<(), CliError> {
        let operation = format!("delete task {name:?}");
        let response = self
            .send_once_result(
                Method::DELETE,
                &operation,
                self.url(&format!("/v1/tasks/{name}")),
                None,
            )
            .await?;
        Self::handle_empty(&operation, response).await
    }
}

/// Returns true for retryable HTTP statuses.
#[must_use]
pub fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
    )
}

/// Returns true for retryable transport errors (connection resets, timeouts).
#[must_use]
pub fn is_retryable_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_body() || error.is_decode()
}

/// Returns true when an HTTP status must never be retried.
#[must_use]
pub const fn is_non_retryable_status(status: u16) -> bool {
    matches!(status, 400 | 404 | 409 | 422)
}

fn append_pagination(url: &mut String, limit: Option<u32>, continue_token: Option<&str>) {
    let mut first = true;
    if let Some(value) = limit {
        url.push_str(&format!("?limit={value}"));
        first = false;
    }
    if let Some(token) = continue_token {
        let encoded = urlencoding(token);
        if first {
            url.push('?');
        } else {
            url.push('&');
        }
        url.push_str(&format!("continue={encoded}"));
    }
}

fn urlencoding(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn page_token<T>(page: &ListResponse<T>) -> Option<String> {
    page.metadata()
        .continue_token()
        .map(|token| token.as_str().to_owned())
}

fn parse_retry_after_from_bytes(bytes: &[u8]) -> Option<Duration> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    let error: ApiError = serde_json::from_slice(bytes).ok()?;
    let seconds = error.details()?.retry_after_seconds?;
    Some(Duration::from_secs(u64::from(seconds)))
}

fn error_from_bytes(operation: &str, status: u16, bytes: &[u8]) -> CliError {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return CliError::transport(
            operation,
            format!("server returned HTTP {status} with an oversized body that was discarded"),
        );
    }
    if let Ok(error) = serde_json::from_slice::<ApiError>(bytes) {
        return CliError::api(operation, status, error);
    }
    let preview = String::from_utf8_lossy(bytes);
    let truncated: String = preview.chars().take(ERROR_PREVIEW_LIMIT).collect();
    CliError::transport(
        operation,
        format!("server returned HTTP {status}: {truncated}"),
    )
}

async fn response_to_error(operation: &str, response: reqwest::Response) -> CliError {
    let status = response.status().as_u16();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return CliError::transport(operation, format!("cannot read error response: {error}"));
        }
    };
    if bytes.len() > MAX_RESPONSE_BYTES {
        return CliError::transport(
            operation,
            format!("server returned HTTP {status} with an oversized body that was discarded"),
        );
    }
    if let Ok(error) = serde_json::from_slice::<ApiError>(&bytes) {
        return CliError::api(operation, status, error);
    }
    let preview = String::from_utf8_lossy(&bytes);
    let truncated: String = preview.chars().take(ERROR_PREVIEW_LIMIT).collect();
    CliError::transport(
        operation,
        format!("server returned HTTP {status}: {truncated}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_classification_matches_the_contract() {
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(StatusCode::CONFLICT));

        for status in [400, 404, 409, 422] {
            assert!(
                is_non_retryable_status(status),
                "{status} must not be retried"
            );
        }
        for status in [502, 503, 504] {
            assert!(!is_non_retryable_status(status), "{status} may be retried");
        }
    }

    #[test]
    fn pagination_appends_valid_query_parameters() {
        let mut url = String::from("http://x/v1/agents");
        append_pagination(&mut url, Some(25), Some("default/bravo"));

        assert_eq!(url, "http://x/v1/agents?limit=25&continue=default%2Fbravo");
    }
}
