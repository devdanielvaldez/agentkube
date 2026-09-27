use crate::{
    CostEstimate, GenerationRequest, GenerationResponse, ProviderCapabilities, ProviderFuture,
    ProviderHealth, StreamEvent,
};
use agentkube_agents::ProviderName;

/// Asynchronous stream of normalized model generation events.
pub trait ModelEventStream: Send {
    /// Returns the next event, or `None` after a terminal event.
    fn next<'a>(&'a mut self) -> ProviderFuture<'a, Option<StreamEvent>>;
}

/// Provider adapter contract implemented by OpenAI, Anthropic, local, and custom backends.
pub trait ModelProvider: Send + Sync {
    /// Returns the stable provider name used by policies and routing.
    fn name(&self) -> &ProviderName;

    /// Returns the provider's immutable model catalog.
    fn capabilities(&self) -> &ProviderCapabilities;

    /// Reports current provider health.
    fn health<'a>(&'a self) -> ProviderFuture<'a, ProviderHealth>;

    /// Estimates maximum request cost without issuing inference.
    fn estimate_cost<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> ProviderFuture<'a, CostEstimate>;

    /// Produces one complete normalized response.
    fn generate<'a>(&'a self, request: GenerationRequest)
    -> ProviderFuture<'a, GenerationResponse>;

    /// Starts incremental generation.
    fn stream<'a>(
        &'a self,
        request: GenerationRequest,
    ) -> ProviderFuture<'a, Box<dyn ModelEventStream>>;
}
