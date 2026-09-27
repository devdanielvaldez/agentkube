use agentkube_agents::ModelName;
use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroU32};

/// Operational health category reported by a provider adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealthStatus {
    /// Requests are expected to succeed normally.
    Healthy,
    /// Requests may succeed with elevated errors or latency.
    Degraded,
    /// The adapter should not receive new traffic.
    Unavailable,
}

/// Current provider health and optional diagnostic detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    status: ProviderHealthStatus,
    message: Option<String>,
}

impl ProviderHealth {
    /// Creates a health observation.
    #[must_use]
    pub fn new(status: ProviderHealthStatus, message: Option<String>) -> Self {
        Self { status, message }
    }

    /// Creates a healthy observation without diagnostics.
    #[must_use]
    pub const fn healthy() -> Self {
        Self {
            status: ProviderHealthStatus::Healthy,
            message: None,
        }
    }

    /// Returns the operational health category.
    #[must_use]
    pub const fn status(&self) -> ProviderHealthStatus {
        self.status
    }

    /// Returns optional provider diagnostics.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Returns whether the provider may receive new traffic.
    #[must_use]
    pub const fn accepts_traffic(&self) -> bool {
        !matches!(self.status, ProviderHealthStatus::Unavailable)
    }
}

/// Capabilities and pricing metadata for one provider model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    context_window: NonZeroU32,
    max_output_tokens: NonZeroU32,
    supports_tools: bool,
    supports_streaming: bool,
    input_micro_usd_per_million_tokens: u64,
    output_micro_usd_per_million_tokens: u64,
}

impl ModelCapabilities {
    /// Creates model capability and list-price metadata.
    #[must_use]
    pub const fn new(
        context_window: NonZeroU32,
        max_output_tokens: NonZeroU32,
        supports_tools: bool,
        supports_streaming: bool,
        input_micro_usd_per_million_tokens: u64,
        output_micro_usd_per_million_tokens: u64,
    ) -> Self {
        Self {
            context_window,
            max_output_tokens,
            supports_tools,
            supports_streaming,
            input_micro_usd_per_million_tokens,
            output_micro_usd_per_million_tokens,
        }
    }

    /// Returns the total input and output context window.
    #[must_use]
    pub const fn context_window(&self) -> NonZeroU32 {
        self.context_window
    }

    /// Returns the maximum output-token request.
    #[must_use]
    pub const fn max_output_tokens(&self) -> NonZeroU32 {
        self.max_output_tokens
    }

    /// Returns whether function/tool calling is supported.
    #[must_use]
    pub const fn supports_tools(&self) -> bool {
        self.supports_tools
    }

    /// Returns whether incremental output is supported.
    #[must_use]
    pub const fn supports_streaming(&self) -> bool {
        self.supports_streaming
    }

    /// Returns input list price in micro-USD per million tokens.
    #[must_use]
    pub const fn input_price(&self) -> u64 {
        self.input_micro_usd_per_million_tokens
    }

    /// Returns output list price in micro-USD per million tokens.
    #[must_use]
    pub const fn output_price(&self) -> u64 {
        self.output_micro_usd_per_million_tokens
    }
}

/// Immutable model catalog advertised by a provider adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    models: BTreeMap<ModelName, ModelCapabilities>,
}

impl ProviderCapabilities {
    /// Creates a non-empty catalog, rejecting duplicate model names.
    pub fn new(
        models: impl IntoIterator<Item = (ModelName, ModelCapabilities)>,
    ) -> Result<Self, CapabilityError> {
        let mut catalog = BTreeMap::new();
        for (name, capabilities) in models {
            if catalog.insert(name.clone(), capabilities).is_some() {
                return Err(CapabilityError::DuplicateModel(name));
            }
        }
        if catalog.is_empty() {
            return Err(CapabilityError::EmptyCatalog);
        }
        Ok(Self { models: catalog })
    }

    /// Returns metadata for a model when it is supported.
    #[must_use]
    pub fn model(&self, name: &ModelName) -> Option<&ModelCapabilities> {
        self.models.get(name)
    }

    /// Iterates over models in stable name order.
    pub fn models(&self) -> impl ExactSizeIterator<Item = (&ModelName, &ModelCapabilities)> {
        self.models.iter()
    }
}

/// Invalid provider capability catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// A provider must advertise at least one model.
    EmptyCatalog,
    /// A model name appears more than once.
    DuplicateModel(ModelName),
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCatalog => formatter.write_str("provider model catalog must not be empty"),
            Self::DuplicateModel(model) => write!(formatter, "duplicate provider model {model}"),
        }
    }
}

impl Error for CapabilityError {}
