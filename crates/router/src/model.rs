use agentkube_agents::{ModelName, ModelPolicy, ProviderName};
use agentkube_core::HumanDuration;
use agentkube_providers::{
    CostEstimate, GenerationRequest, GenerationResponse, ModelEventStream, ProviderError,
    ProviderFuture, StreamEvent,
};
use agentkube_tasks::PrivacyRequirement;
use std::{error::Error, fmt, num::NonZeroU32};

/// Data-handling boundary for a model endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataResidency {
    /// Data leaves the AgentKube-controlled environment.
    Remote,
    /// Remote processing occurs in an approved confidential environment.
    Confidential,
    /// Inference remains on locally controlled infrastructure.
    Local,
}

impl DataResidency {
    pub(crate) const fn satisfies(self, requirement: PrivacyRequirement) -> bool {
        match requirement {
            PrivacyRequirement::Standard => true,
            PrivacyRequirement::Confidential => {
                matches!(self, Self::Confidential | Self::Local)
            }
            PrivacyRequirement::LocalOnly => matches!(self, Self::Local),
        }
    }
}

/// Operational metrics used to rank one provider model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoutingProfile {
    quality_basis_points: u16,
    p95_latency: HumanDuration,
    residency: DataResidency,
}

impl ModelRoutingProfile {
    /// Creates a profile with a quality score from 0 through 10,000.
    pub fn new(
        quality_basis_points: u16,
        p95_latency: HumanDuration,
        residency: DataResidency,
    ) -> Result<Self, RoutingProfileError> {
        if quality_basis_points > 10_000 {
            return Err(RoutingProfileError::QualityOutOfRange(quality_basis_points));
        }
        Ok(Self {
            quality_basis_points,
            p95_latency,
            residency,
        })
    }

    /// Returns normalized quality in basis points.
    #[must_use]
    pub const fn quality_basis_points(&self) -> u16 {
        self.quality_basis_points
    }

    /// Returns observed 95th-percentile latency.
    #[must_use]
    pub const fn p95_latency(&self) -> HumanDuration {
        self.p95_latency
    }

    /// Returns the model's data-handling boundary.
    #[must_use]
    pub const fn residency(&self) -> DataResidency {
        self.residency
    }
}

/// Invalid routing profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingProfileError {
    /// Quality cannot exceed 100.00 percent.
    QualityOutOfRange(u16),
}

impl fmt::Display for RoutingProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QualityOutOfRange(value) => {
                write!(
                    formatter,
                    "quality score {value} exceeds 10000 basis points"
                )
            }
        }
    }
}

impl Error for RoutingProfileError {}

/// Hard filters applied before ranking candidate models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingConstraints {
    privacy: PrivacyRequirement,
    minimum_context_tokens: Option<NonZeroU32>,
    maximum_cost_micro_usd: Option<u64>,
    require_streaming: bool,
}

impl RoutingConstraints {
    /// Creates permissive constraints for standard data.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            privacy: PrivacyRequirement::Standard,
            minimum_context_tokens: None,
            maximum_cost_micro_usd: None,
            require_streaming: false,
        }
    }

    /// Requires a data residency level.
    #[must_use]
    pub const fn with_privacy(mut self, privacy: PrivacyRequirement) -> Self {
        self.privacy = privacy;
        self
    }

    /// Requires at least this context window.
    #[must_use]
    pub const fn with_minimum_context(mut self, tokens: NonZeroU32) -> Self {
        self.minimum_context_tokens = Some(tokens);
        self
    }

    /// Rejects estimates above this request budget.
    #[must_use]
    pub const fn with_maximum_cost(mut self, micro_usd: u64) -> Self {
        self.maximum_cost_micro_usd = Some(micro_usd);
        self
    }

    /// Requires streaming support.
    #[must_use]
    pub const fn requiring_streaming(mut self) -> Self {
        self.require_streaming = true;
        self
    }

    pub(crate) const fn privacy(self) -> PrivacyRequirement {
        self.privacy
    }

    pub(crate) const fn minimum_context(self) -> Option<NonZeroU32> {
        self.minimum_context_tokens
    }

    pub(crate) const fn maximum_cost(self) -> Option<u64> {
        self.maximum_cost_micro_usd
    }

    pub(crate) const fn requires_streaming(self) -> bool {
        self.require_streaming
    }
}

impl Default for RoutingConstraints {
    fn default() -> Self {
        Self::new()
    }
}

/// Model policy, generation payload, and hard routing constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingRequest {
    policy: ModelPolicy,
    generation: GenerationRequest,
    constraints: RoutingConstraints,
}

impl RoutingRequest {
    /// Creates a routing request with permissive constraints.
    #[must_use]
    pub const fn new(policy: ModelPolicy, generation: GenerationRequest) -> Self {
        Self {
            policy,
            generation,
            constraints: RoutingConstraints::new(),
        }
    }

    /// Applies hard routing constraints.
    #[must_use]
    pub const fn with_constraints(mut self, constraints: RoutingConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Returns the model-selection policy.
    #[must_use]
    pub const fn policy(&self) -> &ModelPolicy {
        &self.policy
    }

    /// Returns the provider-neutral generation payload.
    #[must_use]
    pub const fn generation(&self) -> &GenerationRequest {
        &self.generation
    }

    /// Returns hard routing constraints.
    #[must_use]
    pub const fn constraints(&self) -> RoutingConstraints {
        self.constraints
    }
}

/// Auditable model-selection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    provider: ProviderName,
    model: ModelName,
    estimated_cost: CostEstimate,
    profile: ModelRoutingProfile,
}

impl RouteDecision {
    pub(crate) fn new(
        provider: ProviderName,
        model: ModelName,
        estimated_cost: CostEstimate,
        profile: ModelRoutingProfile,
    ) -> Self {
        Self {
            provider,
            model,
            estimated_cost,
            profile,
        }
    }

    /// Returns the selected provider.
    #[must_use]
    pub const fn provider(&self) -> &ProviderName {
        &self.provider
    }

    /// Returns the selected model.
    #[must_use]
    pub const fn model(&self) -> &ModelName {
        &self.model
    }

    /// Returns the provider's preflight cost estimate.
    #[must_use]
    pub const fn estimated_cost(&self) -> CostEstimate {
        self.estimated_cost
    }

    /// Returns the operational profile used for ranking.
    #[must_use]
    pub const fn profile(&self) -> &ModelRoutingProfile {
        &self.profile
    }
}

/// Successful response paired with its auditable route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedResponse {
    decision: RouteDecision,
    response: GenerationResponse,
    prior_failures: Vec<ProviderError>,
}

impl RoutedResponse {
    pub(crate) const fn new(
        decision: RouteDecision,
        response: GenerationResponse,
        prior_failures: Vec<ProviderError>,
    ) -> Self {
        Self {
            decision,
            response,
            prior_failures,
        }
    }

    /// Returns the successful route.
    #[must_use]
    pub const fn decision(&self) -> &RouteDecision {
        &self.decision
    }

    /// Returns normalized provider output.
    #[must_use]
    pub const fn response(&self) -> &GenerationResponse {
        &self.response
    }

    /// Returns retryable provider failures preceding the successful route.
    #[must_use]
    pub fn prior_failures(&self) -> &[ProviderError] {
        &self.prior_failures
    }

    /// Splits route metadata, provider output, and prior retryable failures.
    #[must_use]
    pub fn into_parts(self) -> (RouteDecision, GenerationResponse, Vec<ProviderError>) {
        (self.decision, self.response, self.prior_failures)
    }
}

/// Streaming response paired with its selected route.
pub struct RoutedStream {
    decision: RouteDecision,
    inner: Box<dyn ModelEventStream>,
    prior_failures: Vec<ProviderError>,
}

impl RoutedStream {
    pub(crate) fn new(
        decision: RouteDecision,
        inner: Box<dyn ModelEventStream>,
        prior_failures: Vec<ProviderError>,
    ) -> Self {
        Self {
            decision,
            inner,
            prior_failures,
        }
    }

    /// Returns the selected route.
    #[must_use]
    pub const fn decision(&self) -> &RouteDecision {
        &self.decision
    }

    /// Returns retryable failures encountered before this stream started.
    #[must_use]
    pub fn prior_failures(&self) -> &[ProviderError] {
        &self.prior_failures
    }

    /// Returns the next provider event.
    pub fn next<'a>(&'a mut self) -> ProviderFuture<'a, Option<StreamEvent>> {
        self.inner.next()
    }

    /// Splits route metadata, provider stream, and prior retryable failures.
    #[must_use]
    pub fn into_parts(self) -> (RouteDecision, Box<dyn ModelEventStream>, Vec<ProviderError>) {
        (self.decision, self.inner, self.prior_failures)
    }
}
