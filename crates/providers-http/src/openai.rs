//! OpenAI (and OpenAI-compatible) chat-completions inference.
//!
//! Costs use catalog list prices; token counts always reflect
//! provider-reported usage, including cached input tokens when reported.

use crate::http::{
    self, AdapterError, OPENAI_TIMEOUT, build_client, error_preview, estimate_cost,
    normalize_base_url, role_name, send_error, status_error, tool_definitions,
};
use agentkube_agents::{ModelName, ProviderName, ToolName};
use agentkube_providers::{
    CostEstimate, FinishReason, GenerationRequest, GenerationResponse, GenerationUsage,
    MessageText, ModelCapabilities, ModelEventStream, ModelProvider, ProviderCapabilities,
    ProviderError, ProviderErrorKind, ProviderFuture, ProviderHealth, ProviderHealthStatus,
    StreamEvent, ToolCall, ToolCallId,
};
use serde::Deserialize;

/// OpenAI chat-completions adapter.
pub struct OpenAiProvider {
    name: ProviderName,
    base_url: String,
    api_key: String,
    timeout: std::time::Duration,
    client: reqwest::Client,
    capabilities: ProviderCapabilities,
}

impl OpenAiProvider {
    /// Stable provider name used by policies and routing.
    pub const PROVIDER_NAME: &'static str = "openai";

    /// Creates an adapter with an explicit model catalog.
    ///
    /// The key travels only in the `Authorization` header and never appears
    /// in errors, logs, or diagnostics.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<(ModelName, ModelCapabilities)>,
    ) -> Result<Self, AdapterError> {
        Self::with_timeout(base_url, api_key, models, OPENAI_TIMEOUT)
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

    fn wire_messages(request: &GenerationRequest) -> Vec<serde_json::Value> {
        request
            .messages()
            .iter()
            .map(|message| {
                let mut wire = serde_json::json!({
                    "role": role_name(message.role()),
                    "content": message.content().map_or("", MessageText::as_str),
                });
                if let Some(call_id) = message.tool_call_id() {
                    wire["tool_call_id"] = serde_json::json!(call_id.as_str());
                }
                wire
            })
            .collect()
    }

    async fn completion(
        &self,
        request: &GenerationRequest,
        stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut body = serde_json::json!({
            "model": request.model().as_str(),
            "messages": Self::wire_messages(request),
            "tools": tool_definitions(request),
            "max_tokens": request.max_output_tokens().get(),
            "temperature": f64::from(request.temperature_milli()) / 1000.0,
        });
        if stream {
            body["stream"] = serde_json::json!(true);
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
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

    fn normalize(
        &self,
        request: &GenerationRequest,
        completion: Completion,
    ) -> Result<GenerationResponse, ProviderError> {
        let choice = completion.choices.into_iter().next().ok_or_else(|| {
            self.failed(ProviderErrorKind::Protocol, "openai returned no choices")
        })?;
        let usage = GenerationUsage::new(
            Choice::usage_prompt_tokens(completion.usage.as_ref()),
            Choice::usage_completion_tokens(completion.usage.as_ref()),
            Choice::usage_cached_tokens(completion.usage.as_ref()),
        );
        match choice.finish_reason.as_deref() {
            None | Some("stop") => {
                let content = choice
                    .content()
                    .filter(|text| !text.trim().is_empty())
                    .ok_or_else(|| {
                        self.failed(ProviderErrorKind::Protocol, "openai returned no content")
                    })?;
                let text = MessageText::new(content)
                    .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid openai text"))?;
                GenerationResponse::text(request.model().clone(), text, FinishReason::Stop, usage)
                    .map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid openai response")
                    })
            }
            Some("length") => {
                let content = choice.content().unwrap_or_default().to_owned();
                let text = if content.trim().is_empty() {
                    // A truncated empty completion carries no output; surface
                    // it as missing output rather than an empty result.
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "openai truncated to empty content",
                    ));
                } else {
                    MessageText::new(content).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid openai text")
                    })?
                };
                GenerationResponse::text(request.model().clone(), text, FinishReason::Length, usage)
                    .map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid openai response")
                    })
            }
            Some("tool_calls") | Some("function_call") => {
                let calls = choice.tool_calls();
                if calls.is_empty() {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "openai stopped for tool calls without any",
                    ));
                }
                let mut normalized = Vec::with_capacity(calls.len());
                for call in calls {
                    normalized.push(self.normalize_tool_call(call)?);
                }
                GenerationResponse::tool_calls(request.model().clone(), None, normalized, usage)
                    .map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid openai tool calls")
                    })
            }
            Some("content_filter") => {
                Ok(GenerationResponse::filtered(request.model().clone(), usage))
            }
            Some(_) => {
                Err(self.failed(ProviderErrorKind::Protocol, "unknown openai finish reason"))
            }
        }
    }

    fn normalize_tool_call(&self, call: &WireToolCall) -> Result<ToolCall, ProviderError> {
        let invalid = |message: &str| self.failed(ProviderErrorKind::Protocol, message);
        let id =
            ToolCallId::new(call.id.clone()).map_err(|_| invalid("invalid openai tool call id"))?;
        let name = ToolName::new(call.function.name.clone())
            .map_err(|_| invalid("invalid openai tool name"))?;
        let arguments: serde_json::Value = serde_json::from_str(&call.function.arguments)
            .map_err(|_| invalid("openai tool arguments are not valid JSON"))?;
        ToolCall::new(id, name, arguments).map_err(|_| invalid("invalid openai tool call"))
    }
}

#[derive(Debug, Deserialize)]
struct Completion {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<WireMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Debug, Deserialize)]
struct WireToolCall {
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: WireFunction,
}

#[derive(Debug, Deserialize, Default)]
struct WireFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<PromptDetails>,
}

#[derive(Debug, Deserialize, Default)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: u64,
}

impl Choice {
    fn content(&self) -> Option<&str> {
        self.message
            .as_ref()
            .and_then(|message| message.content.as_deref())
    }

    fn tool_calls(&self) -> &[WireToolCall] {
        self.message
            .as_ref()
            .map_or(&[], |message| message.tool_calls.as_slice())
    }

    fn usage_prompt_tokens(usage: Option<&Usage>) -> u64 {
        usage.map_or(0, |usage| usage.prompt_tokens)
    }

    fn usage_completion_tokens(usage: Option<&Usage>) -> u64 {
        usage.map_or(0, |usage| usage.completion_tokens)
    }

    fn usage_cached_tokens(usage: Option<&Usage>) -> u64 {
        usage.map_or(0, |usage| {
            usage
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |details| details.cached_tokens)
        })
    }
}

impl ModelProvider for OpenAiProvider {
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
                .bearer_auth(&self.api_key)
                .send()
                .await
                .map_err(|error| send_error(&self.name, &error))?;
            let status = response.status().as_u16();
            match status {
                200..=299 => Ok(ProviderHealth::healthy()),
                401 | 403 => Ok(ProviderHealth::new(
                    ProviderHealthStatus::Unavailable,
                    Some("openai authentication rejected".to_owned()),
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
                    format!("model {} is not in the openai catalog", request.model()),
                ));
            }
            let body: Completion = self
                .completion(&request, false)
                .await?
                .json()
                .await
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid openai response"))?;
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
                    format!("model {} is not in the openai catalog", request.model()),
                ));
            }
            let response = self.completion(&request, true).await?;
            Ok(Box::new(OpenAiStream::new(
                self.name.clone(),
                request.model().clone(),
                response,
            )) as Box<dyn ModelEventStream>)
        })
    }
}

/// Incremental OpenAI SSE stream (`data: {...}` lines ending with `[DONE]`).
struct OpenAiStream {
    provider: ProviderName,
    model: ModelName,
    response: Option<reqwest::Response>,
    buffer: Vec<u8>,
    queued: std::collections::VecDeque<String>,
    started: bool,
    pending_usage: Option<GenerationUsage>,
    pending_reason: Option<FinishReason>,
    finished: bool,
}

impl OpenAiStream {
    fn new(provider: ProviderName, model: ModelName, response: reqwest::Response) -> Self {
        Self {
            provider,
            model,
            response: Some(response),
            buffer: Vec::new(),
            queued: std::collections::VecDeque::new(),
            started: false,
            pending_usage: None,
            pending_reason: None,
            finished: false,
        }
    }

    fn failed(&self, kind: ProviderErrorKind, message: impl Into<String>) -> ProviderError {
        ProviderError::new(self.provider.clone(), kind, message)
    }

    async fn next_chunk(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let raw: Vec<u8> = self.buffer.drain(..=position).collect();
                let text = String::from_utf8_lossy(&raw);
                let trimmed = text.trim();
                if trimmed.is_empty() || trimmed.starts_with(':') {
                    continue;
                }
                let Some(payload) = trimmed.strip_prefix("data:") else {
                    return Err(
                        self.failed(ProviderErrorKind::Protocol, "invalid openai stream line")
                    );
                };
                let payload = payload.trim();
                if payload == "[DONE]" {
                    return Ok(Some(SseEvent::Done));
                }
                let chunk: StreamChunk = serde_json::from_str(payload).map_err(|_| {
                    self.failed(ProviderErrorKind::Protocol, "invalid openai stream chunk")
                })?;
                return Ok(Some(SseEvent::Chunk(chunk)));
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
                    let Some(payload) = trimmed.strip_prefix("data:") else {
                        return Err(
                            self.failed(ProviderErrorKind::Protocol, "invalid openai stream line")
                        );
                    };
                    let payload = payload.trim();
                    if payload == "[DONE]" {
                        return Ok(Some(SseEvent::Done));
                    }
                    let chunk: StreamChunk = serde_json::from_str(payload).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid openai stream chunk")
                    })?;
                    return Ok(Some(SseEvent::Chunk(chunk)));
                }
                Err(_) => {
                    return Err(
                        self.failed(ProviderErrorKind::Unavailable, "openai stream interrupted")
                    );
                }
            }
        }
    }
}

enum SseEvent {
    Chunk(StreamChunk),
    Done,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

impl ModelEventStream for OpenAiStream {
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
            if self.pending_usage.take().is_some() {
                self.finished = true;
                return Ok(Some(StreamEvent::Finished(
                    self.pending_reason.take().unwrap_or(FinishReason::Stop),
                )));
            }
            if let Some(content) = self.queued.pop_front() {
                return Ok(Some(StreamEvent::TextDelta(content)));
            }
            loop {
                match self.next_chunk().await? {
                    None => {
                        self.finished = true;
                        return Err(self.failed(
                            ProviderErrorKind::Protocol,
                            "openai stream ended before [DONE]",
                        ));
                    }
                    Some(SseEvent::Done) => {
                        let usage = self.pending_usage.take().unwrap_or_else(|| {
                            // Usage reflects provider-reported counts: without
                            // a usage chunk there is nothing honest to report.
                            GenerationUsage::new(0, 0, 0)
                        });
                        let reason = self.pending_reason.take().unwrap_or(FinishReason::Stop);
                        self.pending_usage = Some(usage);
                        self.pending_reason = Some(reason);
                        return Ok(Some(StreamEvent::Usage(usage)));
                    }
                    Some(SseEvent::Chunk(chunk)) => {
                        if let Some(usage) = chunk.usage {
                            self.pending_usage = Some(GenerationUsage::new(
                                usage.prompt_tokens,
                                usage.completion_tokens,
                                usage
                                    .prompt_tokens_details
                                    .as_ref()
                                    .map_or(0, |details| details.cached_tokens),
                            ));
                        }
                        for choice in &chunk.choices {
                            if let Some(reason) = choice.finish_reason.as_deref() {
                                self.pending_reason = Some(match reason {
                                    "stop" => FinishReason::Stop,
                                    "length" => FinishReason::Length,
                                    "tool_calls" | "function_call" => {
                                        return Err(self.failed(
                                            ProviderErrorKind::Protocol,
                                            "openai streamed tool calls are not supported",
                                        ));
                                    }
                                    "content_filter" => FinishReason::ContentFilter,
                                    _ => {
                                        return Err(self.failed(
                                            ProviderErrorKind::Protocol,
                                            "unknown openai finish reason",
                                        ));
                                    }
                                });
                            }
                            if let Some(content) = choice.delta.content.clone()
                                && !content.is_empty()
                            {
                                self.queued.push_back(content);
                            }
                        }
                        if let Some(content) = self.queued.pop_front() {
                            return Ok(Some(StreamEvent::TextDelta(content)));
                        }
                    }
                }
            }
        })
    }
}
