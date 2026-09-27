use crate::{ChatMessage, MessageText, ToolCall, ToolDefinition};
use agentkube_agents::ModelName;
use agentkube_core::TraceId;
use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroU32};

/// Provider-neutral model generation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRequest {
    model: ModelName,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    max_output_tokens: NonZeroU32,
    temperature_milli: u16,
    trace_id: Option<TraceId>,
}

impl GenerationRequest {
    /// Creates a request with at least one conversation message.
    pub fn new(
        model: ModelName,
        messages: Vec<ChatMessage>,
    ) -> Result<Self, GenerationRequestError> {
        if messages.is_empty() {
            return Err(GenerationRequestError::EmptyMessages);
        }
        Ok(Self {
            model,
            messages,
            tools: Vec::new(),
            max_output_tokens: NonZeroU32::new(1_024).expect("1024 is non-zero"),
            temperature_milli: 1_000,
            trace_id: None,
        })
    }

    /// Advertises tools, rejecting duplicate names.
    pub fn with_tools(
        mut self,
        tools: Vec<ToolDefinition>,
    ) -> Result<Self, GenerationRequestError> {
        let mut names = BTreeSet::new();
        for tool in &tools {
            if !names.insert(tool.name().clone()) {
                return Err(GenerationRequestError::DuplicateTool(tool.name().clone()));
            }
        }
        self.tools = tools;
        Ok(self)
    }

    /// Sets the maximum number of output tokens.
    #[must_use]
    pub const fn with_max_output_tokens(mut self, value: NonZeroU32) -> Self {
        self.max_output_tokens = value;
        self
    }

    /// Sets sampling temperature in thousandths, from 0 through 2000.
    pub fn with_temperature_milli(mut self, value: u16) -> Result<Self, GenerationRequestError> {
        if value > 2_000 {
            return Err(GenerationRequestError::TemperatureOutOfRange(value));
        }
        self.temperature_milli = value;
        Ok(self)
    }

    /// Attaches a trace identifier for end-to-end correlation.
    #[must_use]
    pub const fn with_trace_id(mut self, trace_id: TraceId) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Clones the request for a model selected by automatic routing.
    #[must_use]
    pub fn for_model(&self, model: ModelName) -> Self {
        let mut request = self.clone();
        request.model = model;
        request
    }

    /// Returns the requested provider-specific model name.
    #[must_use]
    pub const fn model(&self) -> &ModelName {
        &self.model
    }

    /// Returns the complete conversation.
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Returns tools advertised for this generation.
    #[must_use]
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    /// Returns the output token ceiling.
    #[must_use]
    pub const fn max_output_tokens(&self) -> NonZeroU32 {
        self.max_output_tokens
    }

    /// Returns sampling temperature in thousandths.
    #[must_use]
    pub const fn temperature_milli(&self) -> u16 {
        self.temperature_milli
    }

    /// Returns the optional trace identifier.
    #[must_use]
    pub const fn trace_id(&self) -> Option<TraceId> {
        self.trace_id
    }
}

/// Invalid generation request construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationRequestError {
    /// A request needs conversation context.
    EmptyMessages,
    /// Tool names must be unique within a request.
    DuplicateTool(agentkube_agents::ToolName),
    /// Temperature is outside the portable 0.000 through 2.000 range.
    TemperatureOutOfRange(u16),
}

impl fmt::Display for GenerationRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMessages => formatter.write_str("generation request needs a message"),
            Self::DuplicateTool(tool) => write!(formatter, "duplicate request tool {tool}"),
            Self::TemperatureOutOfRange(value) => write!(
                formatter,
                "temperature {value} milli is outside the 0..=2000 range"
            ),
        }
    }
}

impl Error for GenerationRequestError {}

/// Reason a provider stopped generating output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FinishReason {
    /// The model reached a natural stop condition.
    Stop,
    /// The configured output-token ceiling was reached.
    Length,
    /// The model requested one or more tool calls.
    ToolCalls,
    /// Provider safety filters interrupted generation.
    ContentFilter,
}

/// Token accounting returned by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GenerationUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
}

impl GenerationUsage {
    /// Creates token usage counters.
    #[must_use]
    pub const fn new(input_tokens: u64, output_tokens: u64, cached_input_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cached_input_tokens,
        }
    }

    /// Returns billed and cached input tokens.
    #[must_use]
    pub const fn input_tokens(self) -> u64 {
        self.input_tokens
    }

    /// Returns generated output tokens.
    #[must_use]
    pub const fn output_tokens(self) -> u64 {
        self.output_tokens
    }

    /// Returns the input tokens served from a provider cache.
    #[must_use]
    pub const fn cached_input_tokens(self) -> u64 {
        self.cached_input_tokens
    }

    /// Returns total tokens when the sum is representable.
    #[must_use]
    pub const fn total_tokens(self) -> Option<u64> {
        self.input_tokens.checked_add(self.output_tokens)
    }
}

/// Complete model generation normalized across providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationResponse {
    model: ModelName,
    text: Option<MessageText>,
    tool_calls: Vec<ToolCall>,
    finish_reason: FinishReason,
    usage: GenerationUsage,
    provider_request_id: Option<String>,
}

impl GenerationResponse {
    /// Creates a text response.
    pub fn text(
        model: ModelName,
        text: MessageText,
        finish_reason: FinishReason,
        usage: GenerationUsage,
    ) -> Result<Self, GenerationResponseError> {
        if finish_reason == FinishReason::ToolCalls {
            return Err(GenerationResponseError::TextMarkedAsToolCalls);
        }
        Ok(Self {
            model,
            text: Some(text),
            tool_calls: Vec::new(),
            finish_reason,
            usage,
            provider_request_id: None,
        })
    }

    /// Creates a response containing tool calls.
    pub fn tool_calls(
        model: ModelName,
        text: Option<MessageText>,
        tool_calls: Vec<ToolCall>,
        usage: GenerationUsage,
    ) -> Result<Self, GenerationResponseError> {
        if tool_calls.is_empty() {
            return Err(GenerationResponseError::MissingToolCalls);
        }
        Ok(Self {
            model,
            text,
            tool_calls,
            finish_reason: FinishReason::ToolCalls,
            usage,
            provider_request_id: None,
        })
    }

    /// Creates an empty response interrupted by provider content filtering.
    #[must_use]
    pub fn filtered(model: ModelName, usage: GenerationUsage) -> Self {
        Self {
            model,
            text: None,
            tool_calls: Vec::new(),
            finish_reason: FinishReason::ContentFilter,
            usage,
            provider_request_id: None,
        }
    }

    /// Attaches the provider's request identifier for diagnostics.
    #[must_use]
    pub fn with_provider_request_id(mut self, value: impl Into<String>) -> Self {
        self.provider_request_id = Some(value.into());
        self
    }

    /// Returns the actual model that produced the response.
    #[must_use]
    pub const fn model(&self) -> &ModelName {
        &self.model
    }

    /// Returns generated text when present.
    #[must_use]
    pub fn generated_text(&self) -> Option<&str> {
        self.text.as_ref().map(MessageText::as_str)
    }

    /// Returns tool calls requested by the model.
    #[must_use]
    pub fn generated_tool_calls(&self) -> &[ToolCall] {
        &self.tool_calls
    }

    /// Returns why generation stopped.
    #[must_use]
    pub const fn finish_reason(&self) -> FinishReason {
        self.finish_reason
    }

    /// Returns provider token accounting.
    #[must_use]
    pub const fn usage(&self) -> GenerationUsage {
        self.usage
    }

    /// Returns the optional provider request identifier.
    #[must_use]
    pub fn provider_request_id(&self) -> Option<&str> {
        self.provider_request_id.as_deref()
    }
}

/// Invalid normalized generation response construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationResponseError {
    /// A tool-call response must contain at least one call.
    MissingToolCalls,
    /// A text-only response cannot claim it stopped for tool calls.
    TextMarkedAsToolCalls,
}

impl fmt::Display for GenerationResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingToolCalls => {
                formatter.write_str("tool-call response needs at least one tool call")
            }
            Self::TextMarkedAsToolCalls => {
                formatter.write_str("text-only response cannot finish with tool calls")
            }
        }
    }
}

impl Error for GenerationResponseError {}

/// Provider list-price estimate in micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CostEstimate {
    input_micro_usd: u64,
    output_micro_usd: u64,
}

impl CostEstimate {
    /// Creates an input/output cost estimate.
    #[must_use]
    pub const fn new(input_micro_usd: u64, output_micro_usd: u64) -> Self {
        Self {
            input_micro_usd,
            output_micro_usd,
        }
    }

    /// Returns estimated input cost.
    #[must_use]
    pub const fn input_micro_usd(self) -> u64 {
        self.input_micro_usd
    }

    /// Returns estimated output cost.
    #[must_use]
    pub const fn output_micro_usd(self) -> u64 {
        self.output_micro_usd
    }

    /// Returns the total estimate when representable.
    #[must_use]
    pub const fn total_micro_usd(self) -> Option<u64> {
        self.input_micro_usd.checked_add(self.output_micro_usd)
    }
}

/// Incremental event emitted by a streaming provider call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// The provider accepted the request and selected a model.
    Started {
        /// Actual model serving the request.
        model: ModelName,
    },
    /// Incremental generated text.
    TextDelta(String),
    /// Complete structured tool call.
    ToolCall(ToolCall),
    /// Final token accounting.
    Usage(GenerationUsage),
    /// Terminal generation reason.
    Finished(FinishReason),
}
