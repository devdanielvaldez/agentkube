use crate::{
    CostEstimate, GenerationRequest, GenerationResponse, ModelEventStream, ModelProvider,
    ProviderCapabilities, ProviderError, ProviderErrorKind, ProviderFuture, ProviderHealth,
    ProviderHealthStatus, ProviderResult, StreamEvent,
};
use agentkube_agents::ProviderName;
use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard},
};

/// Deterministic provider adapter for tests, examples, and offline development.
///
/// Responses are consumed in insertion order. Requests are validated against
/// the model catalog and retained for assertions.
pub struct ScriptedProvider {
    name: ProviderName,
    capabilities: ProviderCapabilities,
    state: Mutex<ScriptedState>,
}

struct ScriptedState {
    health: ProviderHealth,
    responses: VecDeque<ProviderResult<GenerationResponse>>,
    requests: Vec<GenerationRequest>,
}

impl ScriptedProvider {
    /// Creates a healthy provider with no scripted responses.
    #[must_use]
    pub fn new(name: ProviderName, capabilities: ProviderCapabilities) -> Self {
        Self {
            name,
            capabilities,
            state: Mutex::new(ScriptedState {
                health: ProviderHealth::healthy(),
                responses: VecDeque::new(),
                requests: Vec::new(),
            }),
        }
    }

    /// Appends a successful response to the deterministic script.
    pub fn push_response(&self, response: GenerationResponse) -> ProviderResult<()> {
        self.lock()?.responses.push_back(Ok(response));
        Ok(())
    }

    /// Appends a normalized failure to the deterministic script.
    pub fn push_error(
        &self,
        kind: ProviderErrorKind,
        message: impl Into<String>,
    ) -> ProviderResult<()> {
        let error = ProviderError::new(self.name.clone(), kind, message);
        self.lock()?.responses.push_back(Err(error));
        Ok(())
    }

    /// Replaces the health observation returned by [`ModelProvider::health`].
    pub fn set_health(&self, health: ProviderHealth) -> ProviderResult<()> {
        self.lock()?.health = health;
        Ok(())
    }

    /// Returns all valid generation requests received so far.
    pub fn recorded_requests(&self) -> ProviderResult<Vec<GenerationRequest>> {
        Ok(self.lock()?.requests.clone())
    }

    fn lock(&self) -> ProviderResult<MutexGuard<'_, ScriptedState>> {
        self.state.lock().map_err(|_| {
            ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::Internal,
                "scripted provider lock is poisoned",
            )
        })
    }

    fn validate_capabilities(
        &self,
        request: &GenerationRequest,
        streaming: bool,
    ) -> ProviderResult<()> {
        let Some(model) = self.capabilities.model(request.model()) else {
            return Err(ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::ModelNotFound,
                format!("model {} is not in the provider catalog", request.model()),
            ));
        };
        if request.max_output_tokens() > model.max_output_tokens() {
            return Err(ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::InvalidRequest,
                format!(
                    "requested {} output tokens, model maximum is {}",
                    request.max_output_tokens(),
                    model.max_output_tokens()
                ),
            ));
        }
        if !request.tools().is_empty() && !model.supports_tools() {
            return Err(ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::InvalidRequest,
                "model does not support tool calls",
            ));
        }
        if streaming && !model.supports_streaming() {
            return Err(ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::InvalidRequest,
                "model does not support streaming",
            ));
        }
        Ok(())
    }

    fn ensure_available(&self, state: &ScriptedState) -> ProviderResult<()> {
        if state.health.status() == ProviderHealthStatus::Unavailable {
            Err(ProviderError::new(
                self.name.clone(),
                ProviderErrorKind::Unavailable,
                state.health.message().unwrap_or("provider is unavailable"),
            ))
        } else {
            Ok(())
        }
    }

    fn estimated_input_tokens(request: &GenerationRequest) -> u64 {
        let message_bytes = request.messages().iter().fold(0_u64, |total, message| {
            let content_bytes = message
                .content()
                .map_or(0, |content| Self::byte_length(content.as_str()));
            let call_id_bytes = message
                .tool_call_id()
                .map_or(0, |call_id| Self::byte_length(call_id.as_str()));
            let call_bytes = message.tool_calls().iter().fold(0_u64, |subtotal, call| {
                subtotal
                    .saturating_add(Self::byte_length(call.id().as_str()))
                    .saturating_add(Self::byte_length(call.name().as_str()))
                    .saturating_add(Self::byte_length(&call.arguments().to_string()))
            });
            total
                .saturating_add(content_bytes)
                .saturating_add(call_id_bytes)
                .saturating_add(call_bytes)
        });
        let tool_bytes = request.tools().iter().fold(0_u64, |total, tool| {
            total
                .saturating_add(Self::byte_length(tool.name().as_str()))
                .saturating_add(Self::byte_length(tool.description()))
                .saturating_add(Self::byte_length(&tool.input_schema().to_string()))
        });
        (message_bytes.saturating_add(tool_bytes).saturating_add(3) / 4).max(1)
    }

    fn byte_length(value: &str) -> u64 {
        u64::try_from(value.len()).unwrap_or(u64::MAX)
    }

    fn priced_tokens(tokens: u64, price_per_million: u64) -> Option<u64> {
        let numerator = u128::from(tokens)
            .checked_mul(u128::from(price_per_million))?
            .checked_add(999_999)?;
        u64::try_from(numerator / 1_000_000).ok()
    }
}

impl ModelProvider for ScriptedProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn health<'a>(&'a self) -> ProviderFuture<'a, ProviderHealth> {
        Box::pin(async move { Ok(self.lock()?.health.clone()) })
    }

    fn estimate_cost<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> ProviderFuture<'a, CostEstimate> {
        Box::pin(async move {
            self.validate_capabilities(request, false)?;
            let model = self.capabilities.model(request.model()).ok_or_else(|| {
                ProviderError::new(
                    self.name.clone(),
                    ProviderErrorKind::ModelNotFound,
                    "model disappeared from immutable catalog",
                )
            })?;
            let input =
                Self::priced_tokens(Self::estimated_input_tokens(request), model.input_price());
            let output = Self::priced_tokens(
                u64::from(request.max_output_tokens().get()),
                model.output_price(),
            );
            match (input, output) {
                (Some(input), Some(output)) => Ok(CostEstimate::new(input, output)),
                _ => Err(ProviderError::new(
                    self.name.clone(),
                    ProviderErrorKind::Internal,
                    "cost estimate exceeded the supported range",
                )),
            }
        })
    }

    fn generate<'a>(
        &'a self,
        request: GenerationRequest,
    ) -> ProviderFuture<'a, GenerationResponse> {
        Box::pin(async move {
            self.validate_capabilities(&request, false)?;
            let mut state = self.lock()?;
            self.ensure_available(&state)?;
            state.requests.push(request);
            state.responses.pop_front().unwrap_or_else(|| {
                Err(ProviderError::new(
                    self.name.clone(),
                    ProviderErrorKind::Internal,
                    "no scripted response remains",
                ))
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: GenerationRequest,
    ) -> ProviderFuture<'a, Box<dyn ModelEventStream>> {
        Box::pin(async move {
            self.validate_capabilities(&request, true)?;
            let response = self.generate(request).await?;
            let mut events = VecDeque::new();
            events.push_back(StreamEvent::Started {
                model: response.model().clone(),
            });
            if let Some(text) = response.generated_text() {
                events.push_back(StreamEvent::TextDelta(text.to_owned()));
            }
            events.extend(
                response
                    .generated_tool_calls()
                    .iter()
                    .cloned()
                    .map(StreamEvent::ToolCall),
            );
            events.push_back(StreamEvent::Usage(response.usage()));
            events.push_back(StreamEvent::Finished(response.finish_reason()));
            Ok(Box::new(ScriptedEventStream { events }) as Box<dyn ModelEventStream>)
        })
    }
}

struct ScriptedEventStream {
    events: VecDeque<StreamEvent>,
}

impl ModelEventStream for ScriptedEventStream {
    fn next<'a>(&'a mut self) -> ProviderFuture<'a, Option<StreamEvent>> {
        Box::pin(async move { Ok(self.events.pop_front()) })
    }
}
