//! Google Gemini inference (`generateContent`, `streamGenerateContent`).
//!
//! Costs use catalog list prices; token counts always reflect
//! provider-reported usage, including cached input tokens when reported. The
//! API key travels in the `x-goog-api-key` header (never in URLs) so it
//! cannot leak into diagnostics. Multi-turn tool results are rejected loudly:
//! a tool result needs the function name, which our message shape does not
//! retain.

use crate::http::{
    self, AdapterError, GEMINI_TIMEOUT, build_client, error_preview, estimate_cost,
    normalize_base_url, send_error, status_error, tool_definitions,
};
use agentkube_agents::{ModelName, ProviderName};
use agentkube_providers::{
    CostEstimate, FinishReason, GenerationRequest, GenerationResponse, GenerationUsage,
    MessageRole, MessageText, ModelCapabilities, ModelEventStream, ModelProvider,
    ProviderCapabilities, ProviderError, ProviderErrorKind, ProviderFuture, ProviderHealth,
    ProviderHealthStatus, StreamEvent,
};
use serde::Deserialize;

/// Google Gemini adapter.
pub struct GeminiProvider {
    name: ProviderName,
    base_url: String,
    api_key: String,
    timeout: std::time::Duration,
    client: reqwest::Client,
    capabilities: ProviderCapabilities,
}

impl GeminiProvider {
    /// Stable provider name used by policies and routing.
    pub const PROVIDER_NAME: &'static str = "gemini";

    /// Creates an adapter with an explicit model catalog.
    ///
    /// The key travels only in the `x-goog-api-key` header and never appears
    /// in errors, logs, or diagnostics.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<(ModelName, ModelCapabilities)>,
    ) -> Result<Self, AdapterError> {
        Self::with_timeout(base_url, api_key, models, GEMINI_TIMEOUT)
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

    fn wire_contents(
        &self,
        request: &GenerationRequest,
    ) -> Result<(Vec<serde_json::Value>, Option<serde_json::Value>), ProviderError> {
        let invalid = |message: &str| self.failed(ProviderErrorKind::InvalidRequest, message);
        let mut contents = Vec::with_capacity(request.messages().len());
        let mut system = Vec::new();
        for message in request.messages() {
            if !message.tool_calls().is_empty() {
                return Err(invalid("outbound tool calls are not supported"));
            }
            let text = message.content().map(MessageText::as_str).unwrap_or("");
            match message.role() {
                MessageRole::System => system.push(text.to_owned()),
                MessageRole::User => contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{"text": text}],
                })),
                MessageRole::Assistant => contents.push(serde_json::json!({
                    "role": "model",
                    "parts": [{"text": text}],
                })),
                MessageRole::Tool => {
                    return Err(invalid(
                        "tool results need a function name that Gemini tool calls do not retain",
                    ));
                }
            }
        }
        let system = if system.is_empty() {
            None
        } else {
            Some(serde_json::json!({"parts": [{"text": system.join("\n\n")}]}))
        };
        Ok((contents, system))
    }

    async fn completion(
        &self,
        request: &GenerationRequest,
        stream: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let (contents, system) = self.wire_contents(request)?;
        let mut body = serde_json::json!({
            "contents": contents,
            "tools": [{"functionDeclarations": tool_definitions(request)
                .into_iter()
                .map(|tool| tool["function"].clone())
                .collect::<Vec<_>>() }],
            "generationConfig": {
                "maxOutputTokens": request.max_output_tokens().get(),
                "temperature": f64::from(request.temperature_milli()) / 1000.0,
            },
        });
        if let Some(system) = system {
            body["systemInstruction"] = system;
        }
        // Gemini answers 400 when `tools` holds an empty declaration list.
        if body["tools"][0]["functionDeclarations"]
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            body.as_object_mut()
                .expect("request body is an object")
                .remove("tools");
        }
        let endpoint = if stream {
            format!(
                "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.base_url,
                request.model().as_str()
            )
        } else {
            format!(
                "{}/v1beta/models/{}:generateContent",
                self.base_url,
                request.model().as_str()
            )
        };
        let response = self
            .client
            .post(endpoint)
            .header("x-goog-api-key", &self.api_key)
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
        response: GenerateResponse,
    ) -> Result<GenerationResponse, ProviderError> {
        let candidate = response.candidates.into_iter().next().ok_or_else(|| {
            self.failed(ProviderErrorKind::Protocol, "Gemini returned no candidates")
        })?;
        let usage = GenerationUsage::new(
            response
                .usage_metadata
                .as_ref()
                .map_or(0, |usage| usage.prompt_token_count),
            response
                .usage_metadata
                .as_ref()
                .map_or(0, |usage| usage.candidates_token_count),
            response
                .usage_metadata
                .as_ref()
                .map_or(0, |usage| usage.cached_content_token_count),
        );
        let mut text = String::new();
        for part in candidate.content_parts().into_iter().flatten() {
            text.push_str(part);
        }
        match candidate.finish_reason.as_deref() {
            None | Some("STOP") => {
                if text.trim().is_empty() {
                    return Err(
                        self.failed(ProviderErrorKind::Protocol, "Gemini returned no content")
                    );
                }
                let content = MessageText::new(text)
                    .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Gemini text"))?;
                GenerationResponse::text(
                    request.model().clone(),
                    content,
                    FinishReason::Stop,
                    usage,
                )
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Gemini response"))
            }
            Some("MAX_TOKENS") => {
                if text.trim().is_empty() {
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "Gemini truncated to empty content",
                    ));
                }
                let content = MessageText::new(text)
                    .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Gemini text"))?;
                GenerationResponse::text(
                    request.model().clone(),
                    content,
                    FinishReason::Length,
                    usage,
                )
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Gemini response"))
            }
            Some("SAFETY") => Ok(GenerationResponse::filtered(request.model().clone(), usage)),
            Some(_) => {
                Err(self.failed(ProviderErrorKind::Protocol, "unknown Gemini finish reason"))
            }
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct TextPart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ResponseContent {
    #[serde(default)]
    parts: Vec<TextPart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: ResponseContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    #[serde(default)]
    cached_content_token_count: u64,
}

#[derive(Debug, Deserialize, Default)]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

impl Candidate {
    fn content_parts(&self) -> Vec<Option<&str>> {
        self.content
            .parts
            .iter()
            .map(|part| part.text.as_deref())
            .collect()
    }
}

impl ModelProvider for GeminiProvider {
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
                .get(format!("{}/v1beta/models", self.base_url))
                .header("x-goog-api-key", &self.api_key)
                .send()
                .await
                .map_err(|error| send_error(&self.name, &error))?;
            let status = response.status().as_u16();
            match status {
                200..=299 => Ok(ProviderHealth::healthy()),
                401 | 403 => Ok(ProviderHealth::new(
                    ProviderHealthStatus::Unavailable,
                    Some("Gemini authentication rejected".to_owned()),
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
                    format!("model {} is not in the Gemini catalog", request.model()),
                ));
            }
            let body: GenerateResponse = self
                .completion(&request, false)
                .await?
                .json()
                .await
                .map_err(|_| self.failed(ProviderErrorKind::Protocol, "invalid Gemini response"))?;
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
                    format!("model {} is not in the Gemini catalog", request.model()),
                ));
            }
            let response = self.completion(&request, true).await?;
            Ok(Box::new(GeminiStream::new(
                self.name.clone(),
                request.model().clone(),
                response,
            )) as Box<dyn ModelEventStream>)
        })
    }
}

/// Incremental Gemini SSE stream (one full response object per `data:` line).
struct GeminiStream {
    provider: ProviderName,
    model: ModelName,
    response: Option<reqwest::Response>,
    buffer: Vec<u8>,
    queued: std::collections::VecDeque<String>,
    started: bool,
    pending: Option<(GenerationUsage, FinishReason)>,
    finished: bool,
}

impl GeminiStream {
    fn new(provider: ProviderName, model: ModelName, response: reqwest::Response) -> Self {
        Self {
            provider,
            model,
            response: Some(response),
            buffer: Vec::new(),
            queued: std::collections::VecDeque::new(),
            started: false,
            pending: None,
            finished: false,
        }
    }

    fn failed(&self, kind: ProviderErrorKind, message: impl Into<String>) -> ProviderError {
        ProviderError::new(self.provider.clone(), kind, message)
    }

    async fn next_chunk(&mut self) -> Result<Option<GenerateResponse>, ProviderError> {
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let raw: Vec<u8> = self.buffer.drain(..=position).collect();
                let text = String::from_utf8_lossy(&raw);
                let trimmed = text.trim();
                if trimmed.is_empty() || trimmed.starts_with(':') {
                    continue;
                }
                if let Some(payload) = trimmed.strip_prefix("data:") {
                    let payload = payload.trim();
                    if payload == "[DONE]" {
                        return Ok(None);
                    }
                    let chunk: GenerateResponse = serde_json::from_str(payload).map_err(|_| {
                        self.failed(ProviderErrorKind::Protocol, "invalid Gemini stream chunk")
                    })?;
                    return Ok(Some(chunk));
                }
                return Err(self.failed(ProviderErrorKind::Protocol, "invalid Gemini stream line"));
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
                    if trimmed.is_empty() {
                        return Ok(None);
                    }
                    if let Some(payload) = trimmed.strip_prefix("data:") {
                        let payload = payload.trim();
                        if payload == "[DONE]" {
                            return Ok(None);
                        }
                        let chunk: GenerateResponse =
                            serde_json::from_str(payload).map_err(|_| {
                                self.failed(
                                    ProviderErrorKind::Protocol,
                                    "invalid Gemini stream chunk",
                                )
                            })?;
                        return Ok(Some(chunk));
                    }
                    return Err(
                        self.failed(ProviderErrorKind::Protocol, "invalid Gemini stream line")
                    );
                }
                Err(_) => {
                    return Err(
                        self.failed(ProviderErrorKind::Unavailable, "Gemini stream interrupted")
                    );
                }
            }
        }
    }
}

impl ModelEventStream for GeminiStream {
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
            if let Some((_, reason)) = self.pending.take() {
                self.finished = true;
                return Ok(Some(StreamEvent::Finished(reason)));
            }
            if let Some(content) = self.queued.pop_front() {
                return Ok(Some(StreamEvent::TextDelta(content)));
            }
            loop {
                let Some(chunk) = self.next_chunk().await? else {
                    // The connection ended without a terminal chunk: the
                    // stream is truncated, so fail loudly instead of faking
                    // an end.
                    self.finished = true;
                    return Err(self.failed(
                        ProviderErrorKind::Protocol,
                        "Gemini stream ended before a terminal chunk",
                    ));
                };
                // Usage arrives with the terminal chunk; hold the finish until
                // usage has been emitted.
                if chunk.has_terminal_state() {
                    let usage = chunk.reported_usage();
                    let reason = chunk
                        .finish()
                        .map_err(|kind| self.failed(kind, "unknown Gemini finish reason"))?;
                    self.pending = Some((usage, reason));
                    return Ok(Some(StreamEvent::Usage(usage)));
                }
                for candidate in &chunk.candidates {
                    for part in candidate
                        .content_parts()
                        .into_iter()
                        .flatten()
                        .filter(|text| !text.is_empty())
                    {
                        self.queued.push_back(part.to_owned());
                    }
                }
                if let Some(content) = self.queued.pop_front() {
                    return Ok(Some(StreamEvent::TextDelta(content)));
                }
            }
        })
    }
}

impl GenerateResponse {
    fn has_terminal_state(&self) -> bool {
        self.usage_metadata.is_some()
            || self.candidates.iter().any(|candidate| {
                candidate
                    .finish_reason
                    .as_deref()
                    .is_some_and(|reason| !reason.is_empty())
            })
    }

    fn reported_usage(&self) -> GenerationUsage {
        GenerationUsage::new(
            self.usage_metadata
                .as_ref()
                .map_or(0, |usage| usage.prompt_token_count),
            self.usage_metadata
                .as_ref()
                .map_or(0, |usage| usage.candidates_token_count),
            self.usage_metadata
                .as_ref()
                .map_or(0, |usage| usage.cached_content_token_count),
        )
    }

    fn finish(&self) -> Result<FinishReason, ProviderErrorKind> {
        // Single-turn requests yield one candidate; its reason decides.
        match self
            .candidates
            .first()
            .and_then(|candidate| candidate.finish_reason.as_deref())
        {
            None | Some("STOP") => Ok(FinishReason::Stop),
            Some("MAX_TOKENS") => Ok(FinishReason::Length),
            Some("SAFETY") => Ok(FinishReason::ContentFilter),
            Some(_) => Err(ProviderErrorKind::Protocol),
        }
    }
}
