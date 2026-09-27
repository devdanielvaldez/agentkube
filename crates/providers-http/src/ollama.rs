//! Local Ollama inference over HTTP (`/api/chat`, `/api/tags`).
//!
//! Costs are always zero: local inference has no billed usage, so budgets
//! constrain tokens and timeouts while cost limits trivially pass.

use crate::http::{
    self, AdapterError, OLLAMA_TIMEOUT, build_client, error_preview, estimate_cost,
    normalize_base_url, role_name, send_error, status_error, tool_definitions,
};
use agentkube_agents::{ModelName, ProviderName};
use agentkube_providers::{
    CostEstimate, FinishReason, GenerationRequest, GenerationResponse, GenerationUsage,
    MessageText, ModelCapabilities, ModelEventStream, ModelProvider, ProviderCapabilities,
    ProviderError, ProviderErrorKind, ProviderFuture, ProviderHealth, ProviderHealthStatus,
    StreamEvent,
};
use serde::Deserialize;

/// Ollama chat adapter.
pub struct OllamaProvider {
    name: ProviderName,
    base_url: String,
    timeout: std::time::Duration,
    client: reqwest::Client,
    capabilities: ProviderCapabilities,
}

impl OllamaProvider {
    /// Stable provider name used by policies and routing.
    pub const PROVIDER_NAME: &'static str = "ollama";

    /// Creates an adapter with an explicit model catalog.
    pub fn new(
        base_url: impl Into<String>,
        models: Vec<(ModelName, ModelCapabilities)>,
    ) -> Result<Self, AdapterError> {
        Self::with_timeout(base_url, models, OLLAMA_TIMEOUT)
    }

    /// Creates an adapter with a custom request deadline.
    pub fn with_timeout(
        base_url: impl Into<String>,
        models: Vec<(ModelName, ModelCapabilities)>,
        timeout: std::time::Duration,
    ) -> Result<Self, AdapterError> {
        Ok(Self {
            name: ProviderName::new(Self::PROVIDER_NAME).expect("static name is valid"),
            base_url: normalize_base_url(&base_url.into())?,
            timeout,
            client: build_client(timeout)?,
            capabilities: http::build_catalog(models)?,
        })
    }

    /// Returns the configured base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the configured request deadline.
    #[must_use]
    pub const fn timeout(&self) -> std::time::Duration {
        self.timeout
    }

    fn failed(&self, kind: ProviderErrorKind, message: impl Into<String>) -> ProviderError {
        ProviderError::new(self.name.clone(), kind, message)
    }

    async fn completion(
        &self,
        request: &GenerationRequest,
        stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let messages: Vec<serde_json::Value> = request
            .messages()
            .iter()
            .map(|message| {
                serde_json::json!({
                    "role": role_name(message.role()),
                    "content": message.content().map_or("", MessageText::as_str),
                })
            })
            .collect();
        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&serde_json::json!({
                "model": request.model().as_str(),
                "messages": messages,
                "tools": tool_definitions(request),
                "stream": stream,
                "options": {
                    "num_predict": request.max_output_tokens().get(),
                    "temperature": f64::from(request.temperature_milli()) / 1000.0,
                },
            }))
            .send()
            .await
            .map_err(|error| send_error(&self.name, &error))?;
        if response.status().is_success() {
            Ok(response)
        } else {
            let (status, preview) = error_preview(response).await;
            Err(status_error(&self.name, status, &preview))
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    message: Option<ChatMessage>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StreamLine {
    #[serde(default)]
    message: Option<ChatMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

fn finish_reason(done_reason: Option<&str>) -> Result<FinishReason, ProviderErrorKind> {
    match done_reason {
        None | Some("stop") => Ok(FinishReason::Stop),
        Some("length") => Ok(FinishReason::Length),
        Some(_) => Err(ProviderErrorKind::Protocol),
    }
}

impl ModelProvider for OllamaProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn health<'a>(&'a self) -> ProviderFuture<'a, ProviderHealth> {
        Box::pin(async move {
            let response = self
                .client
                .get(format!("{}/api/tags", self.base_url))
                .send()
                .await
                .map_err(|error| send_error(&self.name, &error))?;
            let status = response.status().as_u16();
            match status {
                200..=299 => Ok(ProviderHealth::healthy()),
                401 | 403 => Ok(ProviderHealth::new(
                    ProviderHealthStatus::Unavailable,
                    Some("ollama authentication rejected".to_owned()),
                )),
                _ => Ok(ProviderHealth::new(
                    ProviderHealthStatus::Degraded,
                    Some(format!("unexpected status {status}")),
                )),
            }
        })
    }

    fn estimate_cost<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> ProviderFuture<'a, CostEstimate> {
        Box::pin(async move { estimate_cost(&self.name, &self.capabilities, request) })
    }

    fn generate<'a>(
        &'a self,
        request: GenerationRequest,
    ) -> ProviderFuture<'a, GenerationResponse> {
        Box::pin(async move {
            if self.capabilities.model(request.model()).is_none() {
                return Err(self.failed(
                    ProviderErrorKind::ModelNotFound,
                    format!("model {} is not in the ollama catalog", request.model()),
                ));
            }
            let body: ChatResponse = self
                .completion(&request, false)
                .await?
                .json()
                .await
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid ollama response"))?;
            if body
                .message
                .as_ref()
                .is_some_and(|message| !message.tool_calls.is_empty())
            {
                // Ollama tool calls carry no provider-issued identifiers, so
                // they cannot form a valid ToolCall: report loudly instead of
                // synthesizing identifiers.
                return Err(self.failed(
                    ProviderErrorKind::Protocol,
                    "ollama tool calls are not supported",
                ));
            }
            let content = body
                .message
                .as_ref()
                .and_then(|message| message.content.clone())
                .filter(|content| !content.trim().is_empty())
                .ok_or_else(|| {
                    self.failed(ProviderErrorKind::Protocol, "ollama returned no content")
                })?;
            let text = MessageText::new(content)
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid ollama text"))?;
            GenerationResponse::text(
                request.model().clone(),
                text,
                finish_reason(body.done_reason.as_deref())
                    .map_err(|kind| self.failed(kind, "unknown ollama done reason"))?,
                GenerationUsage::new(
                    body.prompt_eval_count.unwrap_or(0),
                    body.eval_count.unwrap_or(0),
                    0,
                ),
            )
            .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid ollama response"))
        })
    }

    fn stream<'a>(
        &'a self,
        request: GenerationRequest,
    ) -> ProviderFuture<'a, Box<dyn ModelEventStream>> {
        Box::pin(async move {
            if self.capabilities.model(request.model()).is_none() {
                return Err(self.failed(
                    ProviderErrorKind::ModelNotFound,
                    format!("model {} is not in the ollama catalog", request.model()),
                ));
            }
            let response = self.completion(&request, true).await?;
            Ok(Box::new(OllamaStream::new(
                self.name.clone(),
                request.model().clone(),
                response,
            )) as Box<dyn ModelEventStream>)
        })
    }
}

/// Incremental Ollama NDJSON stream (`{"message": ..., "done": ...}` lines).
struct OllamaStream {
    provider: ProviderName,
    model: ModelName,
    response: Option<reqwest::Response>,
    buffer: Vec<u8>,
    started: bool,
    pending_finish: Option<(GenerationUsage, FinishReason)>,
    finished: bool,
}

impl OllamaStream {
    fn new(provider: ProviderName, model: ModelName, response: reqwest::Response) -> Self {
        Self {
            provider,
            model,
            response: Some(response),
            buffer: Vec::new(),
            started: false,
            pending_finish: None,
            finished: false,
        }
    }

    fn failed(&self, kind: ProviderErrorKind, message: impl Into<String>) -> ProviderError {
        ProviderError::new(self.provider.clone(), kind, message)
    }

    async fn next_line(&mut self) -> Result<Option<StreamLine>, ProviderError> {
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let raw: Vec<u8> = self.buffer.drain(..=position).collect();
                let text = String::from_utf8_lossy(&raw);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let line: StreamLine = serde_json::from_str(trimmed).map_err(|_| {
                    self.failed(ProviderErrorKind::Protocol, "invalid ollama stream line")
                })?;
                return Ok(Some(line));
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            match response.chunk().await {
                Ok(Some(bytes)) => self.buffer.extend_from_slice(&bytes),
                Ok(None) => {
                    self.response = None;
                    if self.buffer.is_empty() {
                        return Ok(None);
                    }
                    let raw = std::mem::take(&mut self.buffer);
                    let trimmed = String::from_utf8_lossy(&raw);
                    let trimmed = trimmed.trim();
                    if trimmed.is_empty() {
                        return Ok(None);
                    }
                    let line: StreamLine = serde_json::from_str(trimmed).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid ollama stream line")
                    })?;
                    return Ok(Some(line));
                }
                Err(_) => {
                    return Err(
                        self.failed(ProviderErrorKind::Unavailable, "ollama stream interrupted")
                    );
                }
            }
        }
    }
}

impl ModelEventStream for OllamaStream {
    fn next<'a>(&'a mut self) -> ProviderFuture<'a, Option<StreamEvent>> {
        Box::pin(async move {
            if self.finished {
                return Ok(None);
            }
            if !self.started {
                self.started = true;
                return Ok(Some(StreamEvent::Started {
                    model: self.model.clone(),
                }));
            }
            if let Some((_, reason)) = self.pending_finish.take() {
                self.finished = true;
                return Ok(Some(StreamEvent::Finished(reason)));
            }
            loop {
                let Some(line) = self.next_line().await? else {
                    // The connection ended without a done marker: the stream
                    // is truncated, so fail loudly instead of faking an end.
                    self.finished = true;
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "ollama stream ended before the done marker",
                    ));
                };
                if line
                    .message
                    .as_ref()
                    .is_some_and(|message| !message.tool_calls.is_empty())
                {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "ollama tool calls are not supported",
                    ));
                }
                if let Some(content) = line
                    .message
                    .as_ref()
                    .and_then(|message| message.content.clone())
                    .filter(|content| !content.is_empty())
                {
                    return Ok(Some(StreamEvent::TextDelta(content)));
                }
                if line.done {
                    let usage = GenerationUsage::new(
                        line.prompt_eval_count.unwrap_or(0),
                        line.eval_count.unwrap_or(0),
                        0,
                    );
                    let reason = finish_reason(line.done_reason.as_deref())
                        .map_err(|kind| self.failed(kind, "unknown ollama done reason"))?;
                    self.pending_finish = Some((usage, reason));
                    return Ok(Some(StreamEvent::Usage(usage)));
                }
            }
        })
    }
}
