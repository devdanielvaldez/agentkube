use crate::{
    ProviderRegistry, RouteDecision, RoutedResponse, RoutedStream, RouterError, RouterFuture,
    RoutingRequest, registry::RegisteredProvider,
};
use agentkube_agents::{ModelName, ModelPolicy, OptimizationObjective, ProviderName};
use agentkube_providers::{ModelProvider, ProviderHealthStatus};
use std::{cmp::Ordering, sync::Arc};

/// Asynchronous model-selection and inference-failover contract.
pub trait ModelRouting: Send + Sync {
    /// Selects the best eligible route without issuing inference.
    fn route<'a>(&'a self, request: &'a RoutingRequest) -> RouterFuture<'a, RouteDecision>;

    /// Generates a response, failing over only after retryable errors.
    fn generate<'a>(&'a self, request: RoutingRequest) -> RouterFuture<'a, RoutedResponse>;

    /// Starts streaming, failing over when stream creation returns a retryable error.
    fn stream<'a>(&'a self, request: RoutingRequest) -> RouterFuture<'a, RoutedStream>;
}

/// Default deterministic router backed by a [`ProviderRegistry`].
pub struct ModelRouter {
    registry: Arc<ProviderRegistry>,
}

impl ModelRouter {
    /// Creates a router over a shared provider registry.
    #[must_use]
    pub const fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self { registry }
    }

    /// Returns the shared registry.
    #[must_use]
    pub fn registry(&self) -> Arc<ProviderRegistry> {
        Arc::clone(&self.registry)
    }

    async fn candidates(
        &self,
        request: &RoutingRequest,
        force_streaming: bool,
    ) -> Result<Vec<Candidate>, RouterError> {
        let snapshot = self.registry.snapshot()?;
        self.validate_fixed_policy(request.policy(), &snapshot)?;
        let objectives = match request.policy() {
            ModelPolicy::Fixed { .. } => &[][..],
            ModelPolicy::Auto { optimize_for, .. } => optimize_for.as_slice(),
        };
        let mut candidates = Vec::new();
        let mut provider_errors = Vec::new();

        for (provider_name, registration) in snapshot {
            if !policy_allows_provider(request.policy(), &provider_name) {
                continue;
            }
            let health = match registration.provider.health().await {
                Ok(health) => health,
                Err(error) => {
                    provider_errors.push(error);
                    continue;
                }
            };
            if !health.accepts_traffic() {
                continue;
            }
            let health_rank = match health.status() {
                ProviderHealthStatus::Healthy => 2,
                ProviderHealthStatus::Degraded => 1,
                ProviderHealthStatus::Unavailable => 0,
            };

            for (model_name, profile) in &registration.profiles {
                if !policy_allows_model(request.policy(), &provider_name, model_name) {
                    continue;
                }
                let Some(capabilities) = registration.provider.capabilities().model(model_name)
                else {
                    continue;
                };
                let constraints = request.constraints();
                if !profile.residency().satisfies(constraints.privacy())
                    || constraints
                        .minimum_context()
                        .is_some_and(|required| required > capabilities.context_window())
                    || request.generation().max_output_tokens() > capabilities.max_output_tokens()
                    || (!request.generation().tools().is_empty() && !capabilities.supports_tools())
                    || ((constraints.requires_streaming() || force_streaming)
                        && !capabilities.supports_streaming())
                {
                    continue;
                }

                let generation = request.generation().for_model(model_name.clone());
                let estimate = match registration.provider.estimate_cost(&generation).await {
                    Ok(estimate) => estimate,
                    Err(error) => {
                        provider_errors.push(error);
                        continue;
                    }
                };
                let cost = estimate.total_micro_usd().unwrap_or(u64::MAX);
                if constraints
                    .maximum_cost()
                    .is_some_and(|maximum| cost > maximum)
                {
                    continue;
                }
                candidates.push(Candidate {
                    provider: Arc::clone(&registration.provider),
                    decision: RouteDecision::new(
                        provider_name.clone(),
                        model_name.clone(),
                        estimate,
                        profile.clone(),
                    ),
                    health_rank,
                });
            }
        }

        candidates.sort_by(|left, right| compare_candidates(left, right, objectives));
        if candidates.is_empty() {
            Err(RouterError::NoEligibleModels { provider_errors })
        } else {
            Ok(candidates)
        }
    }

    fn validate_fixed_policy(
        &self,
        policy: &ModelPolicy,
        snapshot: &std::collections::BTreeMap<ProviderName, RegisteredProvider>,
    ) -> Result<(), RouterError> {
        let ModelPolicy::Fixed { provider, model } = policy else {
            return Ok(());
        };
        let registration = snapshot
            .get(provider)
            .ok_or_else(|| RouterError::ProviderNotRegistered(provider.clone()))?;
        if registration.provider.capabilities().model(model).is_none() {
            return Err(RouterError::ModelNotRegistered {
                provider: provider.clone(),
                model: model.clone(),
            });
        }
        Ok(())
    }
}

impl ModelRouting for ModelRouter {
    fn route<'a>(&'a self, request: &'a RoutingRequest) -> RouterFuture<'a, RouteDecision> {
        Box::pin(async move {
            self.candidates(request, false)
                .await?
                .into_iter()
                .next()
                .map(|candidate| candidate.decision)
                .ok_or_else(|| RouterError::NoEligibleModels {
                    provider_errors: Vec::new(),
                })
        })
    }

    fn generate<'a>(&'a self, request: RoutingRequest) -> RouterFuture<'a, RoutedResponse> {
        Box::pin(async move {
            let candidates = self.candidates(&request, false).await?;
            let mut failures = Vec::new();
            for candidate in candidates {
                let generation = request
                    .generation()
                    .for_model(candidate.decision.model().clone());
                match candidate.provider.generate(generation).await {
                    Ok(response) => {
                        return Ok(RoutedResponse::new(candidate.decision, response, failures));
                    }
                    Err(error) if error.is_retryable() => failures.push(error),
                    Err(error) => {
                        return Err(RouterError::ProviderRejected {
                            error,
                            prior_failures: failures,
                        });
                    }
                }
            }
            Err(RouterError::AllProvidersFailed(failures))
        })
    }

    fn stream<'a>(&'a self, request: RoutingRequest) -> RouterFuture<'a, RoutedStream> {
        Box::pin(async move {
            let candidates = self.candidates(&request, true).await?;
            let mut failures = Vec::new();
            for candidate in candidates {
                let generation = request
                    .generation()
                    .for_model(candidate.decision.model().clone());
                match candidate.provider.stream(generation).await {
                    Ok(stream) => {
                        return Ok(RoutedStream::new(candidate.decision, stream, failures));
                    }
                    Err(error) if error.is_retryable() => failures.push(error),
                    Err(error) => {
                        return Err(RouterError::ProviderRejected {
                            error,
                            prior_failures: failures,
                        });
                    }
                }
            }
            Err(RouterError::AllProvidersFailed(failures))
        })
    }
}

struct Candidate {
    provider: Arc<dyn ModelProvider>,
    decision: RouteDecision,
    health_rank: u8,
}

fn policy_allows_provider(policy: &ModelPolicy, provider: &ProviderName) -> bool {
    match policy {
        ModelPolicy::Fixed {
            provider: required, ..
        } => provider == required,
        ModelPolicy::Auto {
            allowed_providers, ..
        } => allowed_providers.is_empty() || allowed_providers.contains(provider),
    }
}

fn policy_allows_model(policy: &ModelPolicy, provider: &ProviderName, model: &ModelName) -> bool {
    match policy {
        ModelPolicy::Fixed {
            provider: required_provider,
            model: required_model,
        } => provider == required_provider && model == required_model,
        ModelPolicy::Auto { .. } => true,
    }
}

fn compare_candidates(
    left: &Candidate,
    right: &Candidate,
    objectives: &[OptimizationObjective],
) -> Ordering {
    let health = right.health_rank.cmp(&left.health_rank);
    if health != Ordering::Equal {
        return health;
    }
    for objective in objectives {
        let ordering = match objective {
            OptimizationObjective::Quality => right
                .decision
                .profile()
                .quality_basis_points()
                .cmp(&left.decision.profile().quality_basis_points()),
            OptimizationObjective::Cost => route_cost(left).cmp(&route_cost(right)),
            OptimizationObjective::Latency => left
                .decision
                .profile()
                .p95_latency()
                .cmp(&right.decision.profile().p95_latency()),
            OptimizationObjective::Privacy => right
                .decision
                .profile()
                .residency()
                .cmp(&left.decision.profile().residency()),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.decision
        .provider()
        .cmp(right.decision.provider())
        .then_with(|| left.decision.model().cmp(right.decision.model()))
}

fn route_cost(candidate: &Candidate) -> u64 {
    candidate
        .decision
        .estimated_cost()
        .total_micro_usd()
        .unwrap_or(u64::MAX)
}
