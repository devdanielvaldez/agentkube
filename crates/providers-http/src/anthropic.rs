//! Anthropic Messages API inference (`/v1/messages`, `/v1/models`).
//!
//! Costs use catalog list prices; token counts always reflect
//! provider-reported usage, including cached input tokens when reported.
//! Outbound tool calls and multi-turn tool results beyond a single
//! `tool_result` block are rejected loudly instead of being downgraded.

use crate::http::{
    self, ANTHROPIC_TIMEOUT, AdapterError, build_client, error_preview, estimate_cost,
    normalize_base_url, send_error, status_error, tool_definitions,
};
use agentkube_agents::{ModelName, ProviderName, ToolName};
use agentkube_providers::{
    CostEstimate, FinishReason, GenerationRequest, GenerationResponse, GenerationUsage,
    MessageRole, MessageText, ModelCapabilities, ModelEventStream, ModelProvider,
    ProviderCapabilities, ProviderError, ProviderErrorKind, ProviderFuture, ProviderHealth,
    ProviderHealthStatus, StreamEvent, ToolCall, ToolCallId,
};
use serde::Deserialize;

/// Anthropic API version pinned by this adapter.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic Messages API adapter.
pub struct AnthropicProvider {
    name: ProviderName,
    base_url: String,
    api_key: String,
    timeout: std::time::Duration,
    client: reqwest::Client,
    capabilities: ProviderCapabilities,
}

impl AnthropicProvider {
    /// Stable provider name used by policies and routing.
    pub const PROVIDER_NAME: &'static str = "anthropic";

    /// Creates an adapter with an explicit model catalog.
    ///
    /// The key travels only in the `x-api-key` header and never appears in
    /// errors, logs, or diagnostics.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<(ModelName, ModelCapabilities)>,
    ) -> Result<Self, AdapterError> {
        Self::with_timeout(base_url, api_key, models, ANTHROPIC_TIMEOUT)
    }

    /// Creates an adapter with a custom request deadline.
    pub fn with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<(ModelName, ModelCapabilities)>,
        timeout: std::time::Duration,
    ) -> Result<Self, AdapterError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(AdapterError::EmptyApiKey);
        }
        Ok(Self {
            name: ProviderName::new(Self::PROVIDER_NAME).expect("static name is valid"),
            base_url: normalize_base_url(&base_url.into())?,
            api_key,
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

    fn wire_messages(
        &self,
        request: &GenerationRequest,
    ) -> Result<(Vec<serde_json::Value>, Option<String>), ProviderError> {
        let invalid = |message: &str| self.failed(ProviderErrorKind::InvalidRequest, message);
        let mut messages = Vec::with_capacity(request.messages().len());
        let mut system = Vec::new();
        for message in request.messages() {
            if !message.tool_calls().is_empty() {
                return Err(invalid("outbound tool calls are not supported"));
            }
            let text = message.content().map(MessageText::as_str).unwrap_or("");
            match message.role() {
                MessageRole::System => system.push(text.to_owned()),
                MessageRole::User => messages.push(serde_json::json!({
                    "role": "user",
                    "content": text,
                })),
                MessageRole::Assistant => messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": text,
                })),
                MessageRole::Tool => {
                    let call_id = message
                        .tool_call_id()
                        .ok_or_else(|| invalid("tool result without a call identifier"))?;
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": call_id.as_str(),
                            "content": text,
                        }],
                    }));
                }
            }
        }
        let system = if system.is_empty() {
            None
        } else {
            Some(system.join("\n\n"))
        };
        Ok((messages, system))
    }

    async fn completion(
        &self,
        request: &GenerationRequest,
        stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        if request.temperature_milli() > 1000 {
            return Err(self.failed(
                ProviderErrorKind::InvalidRequest,
                "temperature exceeds the Anthropic maximum of 1.0",
            ));
        }
        let (messages, system) = self.wire_messages(request)?;
        let mut body = serde_json::json!({
            "model": request.model().as_str(),
            "max_tokens": request.max_output_tokens().get(),
            "messages": messages,
            "tools": tool_definitions(request),
            "temperature": f64::from(request.temperature_milli()) / 1000.0,
            "stream": stream,
        });
        if let Some(system) = system {
            body["system"] = serde_json::json!(system);
        }
        // Anthropic answers 400 when `tools` is present but empty.
        if body["tools"].as_array().is_some_and(Vec::is_empty) {
            body.as_object_mut()
                .expect("request body is an object")
                .remove("tools");
        }
        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|error| send_error(&self.name, &error))?;
        if response.status().is_success() {
            return Ok(response);
        }
        if response.status().as_u16() == 429 {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .and_then(|seconds| format!("{seconds}s").parse().ok());
            let (_, preview) = error_preview(response).await;
            let preview: String = preview.chars().take(300).collect();
            return Err(ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::RateLimited { retry_after },
                format!("HTTP 429: {preview}"),
            ));
        }
        let (status, preview) = error_preview(response).await;
        Err(status_error(&self.name, status, &preview))
    }

    fn normalize(
        &self,
        request: &GenerationRequest,
        message: MessageResponse,
    ) -> Result<GenerationResponse, ProviderError> {
        let usage = GenerationUsage::new(
            message.usage.input_tokens,
            message.usage.output_tokens,
            message.usage.cache_read_input_tokens,
        );
        let mut text = String::new();
        let mut calls = Vec::new();
        for block in &message.content {
            match block.kind.as_str() {
                "text" => {
                    if let Some(part) = block.text.as_deref() {
                        text.push_str(part);
                    }
                }
                "tool_use" => {
                    let id = block
                        .id
                        .clone()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| {
                            self.failed(ProviderErrorKind::Protocol, "tool call without identifier")
                        })?;
                    let id = ToolCallId::new(id).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid tool call id")
                    })?;
                    let name = block
                        .name
                        .clone()
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| {
                            self.failed(ProviderErrorKind::Protocol, "tool call without name")
                        })?;
                    let name = ToolName::new(name).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid tool name")
                    })?;
                    let arguments = block.input.clone().unwrap_or_else(|| serde_json::json!({}));
                    calls.push(ToolCall::new(id, name, arguments).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid tool call")
                    })?);
                }
                _ => {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "unknown Anthropic content block",
                    ));
                }
            }
        }
        match message.stop_reason.as_deref() {
            None | Some("end_turn") | Some("stop_sequence") => {
                if text.trim().is_empty() && calls.is_empty() {
                    return Err(
                        self.failed(ProviderErrorKind::Protocol, "Anthropic returned no content")
                    );
                }
                if !calls.is_empty() {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "Anthropic stopped without tool_use for tool calls",
                    ));
                }
                let content = MessageText::new(text).map_err(|_| {
                    self.failed(ProviderErrorKind::Protocol, "invalid Anthropic text")
                })?;
                GenerationResponse::text(
                    request.model().clone(),
                    content,
                    FinishReason::Stop,
                    usage,
                )
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Anthropic response"))
            }
            Some("max_tokens") => {
                if text.trim().is_empty() {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "Anthropic truncated to empty content",
                    ));
                }
                let content = MessageText::new(text).map_err(|_| {
                    self.failed(ProviderErrorKind::Protocol, "invalid Anthropic text")
                })?;
                GenerationResponse::text(
                    request.model().clone(),
                    content,
                    FinishReason::Length,
                    usage,
                )
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Anthropic response"))
            }
            Some("tool_use") => {
                if calls.is_empty() {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "Anthropic stopped for tool calls without any",
                    ));
                }
                GenerationResponse::tool_calls(request.model().clone(), None, calls, usage).map_err(
                    |_| self.failed(ProviderErrorKind::Protocol, "invalid Anthropic tool calls"),
                )
            }
            Some("refusal") => Ok(GenerationResponse::filtered(request.model().clone(), usage)),
            Some(_) => {
                Err(self.failed(ProviderErrorKind::Protocol, "unknown Anthropic stop reason"))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct MessageResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize, Default)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

impl ModelProvider for AnthropicProvider {
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
                .get(format!("{}/v1/models", self.base_url))
                .header("x-api-key", &self.api_key)
                .send()
                .await
                .map_err(|error| send_error(&self.name, &error))?;
            let status = response.status().as_u16();
            match status {
                200..=299 => Ok(ProviderHealth::healthy()),
                401 | 403 => Ok(ProviderHealth::new(
                    ProviderHealthStatus::Unavailable,
                    Some("Anthropic authentication rejected".to_owned()),
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
                    format!("model {} is not in the Anthropic catalog", request.model()),
                ));
            }
            let body: MessageResponse = self
                .completion(&request, false)
                .await?
                .json()
                .await
                .map_err(|_| {
                    self.failed(ProviderErrorKind::Protocol, "invalid Anthropic response")
                })?;
            self.normalize(&request, body)
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
                    format!("model {} is not in the Anthropic catalog", request.model()),
                ));
            }
            let response = self.completion(&request, true).await?;
            Ok(Box::new(AnthropicStream::new(
                self.name.clone(),
                request.model().clone(),
                response,
            )) as Box<dyn ModelEventStream>)
        })
    }
}

/// Incremental Anthropic SSE stream (`event:` + `data:` lines).
struct AnthropicStream {
    provider: ProviderName,
    model: ModelName,
    response: Option<reqwest::Response>,
    buffer: Vec<u8>,
    queued: std::collections::VecDeque<String>,
    started: bool,
    input_usage: Option<GenerationUsage>,
    pending_reason: Option<FinishReason>,
    finished: bool,
}

impl AnthropicStream {
    fn new(provider: ProviderName, model: ModelName, response: reqwest::Response) -> Self {
        Self {
            provider,
            model,
            response: Some(response),
            buffer: Vec::new(),
            queued: std::collections::VecDeque::new(),
            started: false,
            input_usage: None,
            pending_reason: None,
            finished: false,
        }
    }

    fn failed(&self, kind: ProviderErrorKind, message: impl Into<String>) -> ProviderError {
        ProviderError::new(self.provider.clone(), kind, message)
    }

    async fn next_event(&mut self) -> Result<Option<(String, String)>, ProviderError> {
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let raw: Vec<u8> = self.buffer.drain(..=position).collect();
                let text = String::from_utf8_lossy(&raw);
                let trimmed = text.trim();
                if trimmed.is_empty() || trimmed.starts_with(':') {
                    continue;
                }
                // SSE events arrive as `event:` / `data:` line pairs; collect
                // the data lines belonging to the most recent event type.
                if let Some(kind) = trimmed.strip_prefix("event:") {
                    let kind = kind.trim().to_owned();
                    let mut payload = String::new();
                    let payload = loop {
                        match self.next_data_line().await? {
                            Some(DataLine::Payload(line)) => payload.push_str(&line),
                            Some(DataLine::Blank) | Some(DataLine::End) => break payload,
                            None => {
                                return Err(self.failed(
                                    ProviderErrorKind::Protocol,
                                    "Anthropic stream ended inside an event",
                                ));
                            }
                        }
                    };
                    return Ok(Some((kind, payload)));
                } else if let Some(payload) = trimmed.strip_prefix("data:") {
                    return Ok(Some((String::new(), payload.trim().to_owned())));
                } else {
                    return Err(
                        self.failed(ProviderErrorKind::Protocol, "invalid Anthropic stream line")
                    );
                }
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
                    let trimmed = trimmed.trim().to_owned();
                    if trimmed.is_empty() || trimmed.starts_with(':') {
                        return Ok(None);
                    }
                    // A trailing data line without its blank terminator still
                    // counts when the connection closes cleanly afterwards.
                    if let Some(payload) = trimmed.strip_prefix("data:") {
                        return Ok(Some((String::new(), payload.trim().to_owned())));
                    }
                    return Err(
                        self.failed(ProviderErrorKind::Protocol, "invalid Anthropic stream line")
                    );
                }
                Err(_) => {
                    return Err(self.failed(
                        ProviderErrorKind::Unavailable,
                        "Anthropic stream interrupted",
                    ));
                }
            }
        }
    }

    async fn next_data_line(&mut self) -> Result<Option<DataLine>, ProviderError> {
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let raw: Vec<u8> = self.buffer.drain(..=position).collect();
                let text = String::from_utf8_lossy(&raw);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Ok(Some(DataLine::Blank));
                }
                if trimmed.starts_with(':') {
                    continue;
                }
                if let Some(payload) = trimmed.strip_prefix("data:") {
                    return Ok(Some(DataLine::Payload(payload.trim().to_owned())));
                }
                return Err(
                    self.failed(ProviderErrorKind::Protocol, "invalid Anthropic stream line")
                );
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(Some(DataLine::End));
            };
            match response.chunk().await {
                Ok(Some(bytes)) => self.buffer.extend_from_slice(&bytes),
                Ok(None) => {
                    self.response = None;
                    if self.buffer.is_empty() {
                        return Ok(Some(DataLine::End));
                    }
                    let raw = std::mem::take(&mut self.buffer);
                    let trimmed = String::from_utf8_lossy(&raw);
                    let trimmed = trimmed.trim().to_owned();
                    if trimmed.is_empty() {
                        return Ok(Some(DataLine::End));
                    }
                    return match trimmed.strip_prefix("data:") {
                        Some(payload) => Ok(Some(DataLine::Payload(payload.trim().to_owned()))),
                        None => Err(self
                            .failed(ProviderErrorKind::Protocol, "invalid Anthropic stream line")),
                    };
                }
                Err(_) => {
                    return Err(self.failed(
                        ProviderErrorKind::Unavailable,
                        "Anthropic stream interrupted",
                    ));
                }
            }
        }
    }
}

enum DataLine {
    Payload(String),
    Blank,
    End,
}

#[derive(Debug, Deserialize)]
struct StreamMessage {
    #[serde(default)]
    message: StreamStartMessage,
}

#[derive(Debug, Deserialize, Default)]
struct StreamStartMessage {
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct StreamContent {
    #[serde(default)]
    delta: Option<StreamDeltaContent>,
}

#[derive(Debug, Deserialize)]
struct StreamDeltaContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamError {
    #[serde(default)]
    error: StreamErrorBody,
}

#[derive(Debug, Deserialize, Default)]
struct StreamErrorBody {
    #[serde(default)]
    message: String,
}

impl ModelEventStream for AnthropicStream {
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
            if let Some(content) = self.queued.pop_front() {
                return Ok(Some(StreamEvent::TextDelta(content)));
            }
            loop {
                let Some((kind, payload)) = self.next_event().await? else {
                    self.finished = true;
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "Anthropic stream ended before message_stop",
                    ));
                };
                match kind.as_str() {
                    "message_start" => {
                        let start: StreamMessage =
                            serde_json::from_str(&payload).map_err(|_| {
                                self.failed(ProviderErrorKind::Protocol, "invalid stream start")
                            })?;
                        if let Some(usage) = start.message.usage {
                            self.input_usage = Some(GenerationUsage::new(
                                usage.input_tokens,
                                0,
                                usage.cache_read_input_tokens,
                            ));
                        }
                    }
                    "content_block_delta" => {
                        let delta: StreamContent =
                            serde_json::from_str(&payload).map_err(|_| {
                                self.failed(ProviderErrorKind::Protocol, "invalid stream delta")
                            })?;
                        if let Some(delta) = delta.delta {
                            match delta.kind.as_str() {
                                "text_delta" => {
                                    if let Some(text) = delta.text
                                        && !text.is_empty()
                                    {
                                        self.queued.push_back(text);
                                    }
                                }
                                _ => {
                                    return Err(self.failed(
                                        ProviderErrorKind::Protocol,
                                        "Anthropic streamed tool calls are not supported",
                                    ));
                                }
                            }
                        }
                        if let Some(content) = self.queued.pop_front() {
                            return Ok(Some(StreamEvent::TextDelta(content)));
                        }
                    }
                    "message_delta" => {
                        let delta: StreamDelta = serde_json::from_str(&payload).map_err(|_| {
                            self.failed(ProviderErrorKind::Protocol, "invalid stream delta")
                        })?;
                        let reason = match delta.stop_reason.as_deref() {
                            None | Some("end_turn") | Some("stop_sequence") => FinishReason::Stop,
                            Some("max_tokens") => FinishReason::Length,
                            Some("tool_use") => {
                                return Err(self.failed(
                                    ProviderErrorKind::Protocol,
                                    "Anthropic streamed tool calls are not supported",
                                ));
                            }
                            Some("refusal") => FinishReason::ContentFilter,
                            Some(_) => {
                                return Err(self.failed(
                                    ProviderErrorKind::Protocol,
                                    "unknown Anthropic stop reason",
                                ));
                            }
                        };
                        let output = delta.usage.map_or(0, |usage| usage.output_tokens);
                        let input = self
                            .input_usage
                            .unwrap_or_else(|| GenerationUsage::new(0, 0, 0));
                        let usage = GenerationUsage::new(
                            input.input_tokens(),
                            output,
                            input.cached_input_tokens(),
                        );
                        self.pending_reason = Some(reason);
                        return Ok(Some(StreamEvent::Usage(usage)));
                    }
                    "message_stop" => {
                        let reason = self.pending_reason.take().unwrap_or(FinishReason::Stop);
                        self.finished = true;
                        return Ok(Some(StreamEvent::Finished(reason)));
                    }
                    "ping" | "" => {}
                    "error" => {
                        let error: StreamError = serde_json::from_str(&payload).map_err(|_| {
                            self.failed(ProviderErrorKind::Protocol, "invalid stream error")
                        })?;
                        return Err(self.failed(
                            ProviderErrorKind::Protocol,
                            format!("Anthropic stream error: {}", error.error.message),
                        ));
                    }
                    _ => {
                        return Err(self.failed(
                            ProviderErrorKind::Protocol,
                            "unknown Anthropic stream event",
                        ));
                    }
                }
            }
        })
    }
}
