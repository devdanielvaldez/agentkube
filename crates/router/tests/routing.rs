use agentkube_agents::{ModelName, ModelPolicy, OptimizationObjective, ProviderName, ToolName};
use agentkube_providers::{
    ChatMessage, FinishReason, GenerationRequest, GenerationResponse, GenerationUsage, MessageText,
    ModelCapabilities, ModelProvider, ProviderCapabilities, ProviderErrorKind, ProviderHealth,
    ProviderHealthStatus, ScriptedProvider, StreamEvent, ToolDefinition,
};
use agentkube_router::{
    DataResidency, ModelRouter, ModelRouting, ModelRoutingProfile, ProviderRegistry, RegistryError,
    RouterError, RoutingConstraints, RoutingRequest,
};
use agentkube_tasks::PrivacyRequirement;
use serde_json::json;
use std::{
    future::Future,
    num::NonZeroU32,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("in-memory routing dependency unexpectedly returned pending"),
    }
}

fn name(value: &str) -> ProviderName {
    ProviderName::new(value).unwrap()
}

fn model(value: &str) -> ModelName {
    ModelName::new(value).unwrap()
}

fn profile(quality: u16, latency: &str, residency: DataResidency) -> ModelRoutingProfile {
    ModelRoutingProfile::new(quality, latency.parse().unwrap(), residency).unwrap()
}

fn provider(
    provider_name: &str,
    model_name: &str,
    input_price: u64,
    output_price: u64,
    tools: bool,
    streaming: bool,
) -> Arc<ScriptedProvider> {
    let capabilities = ProviderCapabilities::new([(
        model(model_name),
        ModelCapabilities::new(
            NonZeroU32::new(128_000).unwrap(),
            NonZeroU32::new(8_192).unwrap(),
            tools,
            streaming,
            input_price,
            output_price,
        ),
    )])
    .unwrap();
    Arc::new(ScriptedProvider::new(name(provider_name), capabilities))
}

fn register(
    registry: &ProviderRegistry,
    provider: &Arc<ScriptedProvider>,
    model_name: &str,
    routing_profile: ModelRoutingProfile,
) {
    let adapter: Arc<dyn ModelProvider> = provider.clone();
    registry
        .register(adapter, [(model(model_name), routing_profile)])
        .unwrap();
}

fn generation() -> GenerationRequest {
    GenerationRequest::new(
        model("routing-placeholder"),
        vec![ChatMessage::user(
            MessageText::new("Solve this task").unwrap(),
        )],
    )
    .unwrap()
    .with_max_output_tokens(NonZeroU32::new(100).unwrap())
}

fn auto_request(objectives: &[OptimizationObjective]) -> RoutingRequest {
    RoutingRequest::new(
        ModelPolicy::automatic([], objectives.iter().copied()).unwrap(),
        generation(),
    )
}

fn response(model_name: &str, value: &str) -> GenerationResponse {
    GenerationResponse::text(
        model(model_name),
        MessageText::new(value).unwrap(),
        FinishReason::Stop,
        GenerationUsage::new(10, 5, 0),
    )
    .unwrap()
}

#[test]
fn registry_requires_an_exact_profile_catalog_and_unique_provider_names() {
    let registry = ProviderRegistry::new();
    let adapter = provider("alpha", "alpha-model", 1, 1, true, true);
    let erased: Arc<dyn ModelProvider> = adapter.clone();
    assert!(matches!(
        registry.register(erased, []),
        Err(RegistryError::MissingProfile(_))
    ));

    register(
        &registry,
        &adapter,
        "alpha-model",
        profile(8_000, "1s", DataResidency::Remote),
    );
    let duplicate: Arc<dyn ModelProvider> = adapter;
    assert!(matches!(
        registry.register(
            duplicate,
            [(
                model("alpha-model"),
                profile(8_000, "1s", DataResidency::Remote)
            )]
        ),
        Err(RegistryError::DuplicateProvider(_))
    ));
}

#[test]
fn fixed_policy_selects_exactly_the_requested_provider_and_model() {
    let registry = Arc::new(ProviderRegistry::new());
    let alpha = provider("alpha", "alpha-model", 2_000_000, 2_000_000, true, true);
    register(
        &registry,
        &alpha,
        "alpha-model",
        profile(7_000, "2s", DataResidency::Remote),
    );
    let router = ModelRouter::new(registry);
    let request = RoutingRequest::new(
        ModelPolicy::fixed(name("alpha"), model("alpha-model")),
        generation(),
    );

    let decision = ready(router.route(&request)).unwrap();

    assert_eq!(decision.provider().as_str(), "alpha");
    assert_eq!(decision.model().as_str(), "alpha-model");
}

#[test]
fn automatic_routing_honors_objective_precedence() {
    let registry = Arc::new(ProviderRegistry::new());
    let quality = provider("quality", "quality-model", 9_000_000, 9_000_000, true, true);
    let cheap = provider("cheap", "cheap-model", 1, 1, true, true);
    register(
        &registry,
        &quality,
        "quality-model",
        profile(9_500, "4s", DataResidency::Remote),
    );
    register(
        &registry,
        &cheap,
        "cheap-model",
        profile(7_500, "1s", DataResidency::Remote),
    );
    let router = ModelRouter::new(registry);

    let quality_first = ready(router.route(&auto_request(&[
        OptimizationObjective::Quality,
        OptimizationObjective::Cost,
    ])))
    .unwrap();
    let cost_first = ready(router.route(&auto_request(&[
        OptimizationObjective::Cost,
        OptimizationObjective::Quality,
    ])))
    .unwrap();

    assert_eq!(quality_first.provider().as_str(), "quality");
    assert_eq!(cost_first.provider().as_str(), "cheap");
}

#[test]
fn routing_filters_health_privacy_context_budget_and_tools() {
    let registry = Arc::new(ProviderRegistry::new());
    let remote = provider("remote", "remote-model", 1, 1, true, true);
    let local = provider("local", "local-model", 0, 0, false, true);
    remote
        .set_health(ProviderHealth::new(
            ProviderHealthStatus::Unavailable,
            Some("outage".to_owned()),
        ))
        .unwrap();
    register(
        &registry,
        &remote,
        "remote-model",
        profile(10_000, "100ms", DataResidency::Remote),
    );
    register(
        &registry,
        &local,
        "local-model",
        profile(7_000, "2s", DataResidency::Local),
    );
    let router = ModelRouter::new(registry);
    let constraints = RoutingConstraints::new()
        .with_privacy(PrivacyRequirement::LocalOnly)
        .with_minimum_context(NonZeroU32::new(64_000).unwrap())
        .with_maximum_cost(0)
        .requiring_streaming();
    let request = auto_request(&[OptimizationObjective::Quality]).with_constraints(constraints);

    let decision = ready(router.route(&request)).unwrap();
    assert_eq!(decision.provider().as_str(), "local");

    let tool = ToolDefinition::new(
        ToolName::new("search").unwrap(),
        "Search",
        json!({"type": "object"}),
    )
    .unwrap();
    let tool_request = RoutingRequest::new(
        ModelPolicy::automatic([], [OptimizationObjective::Cost]).unwrap(),
        generation().with_tools(vec![tool]).unwrap(),
    )
    .with_constraints(constraints);
    assert!(matches!(
        ready(router.route(&tool_request)),
        Err(RouterError::NoEligibleModels { .. })
    ));
}

#[test]
fn generation_fails_over_after_retryable_provider_errors() {
    let registry = Arc::new(ProviderRegistry::new());
    let primary = provider("primary", "primary-model", 1, 1, true, true);
    let fallback = provider("fallback", "fallback-model", 1, 1, true, true);
    primary
        .push_error(ProviderErrorKind::RateLimited { retry_after: None }, "busy")
        .unwrap();
    fallback
        .push_response(response("fallback-model", "recovered"))
        .unwrap();
    register(
        &registry,
        &primary,
        "primary-model",
        profile(9_000, "1s", DataResidency::Remote),
    );
    register(
        &registry,
        &fallback,
        "fallback-model",
        profile(8_000, "1s", DataResidency::Remote),
    );
    let router = ModelRouter::new(registry);

    let routed = ready(router.generate(auto_request(&[OptimizationObjective::Quality]))).unwrap();

    assert_eq!(routed.decision().provider().as_str(), "fallback");
    assert_eq!(routed.response().generated_text(), Some("recovered"));
    assert_eq!(routed.prior_failures().len(), 1);
    assert_eq!(primary.recorded_requests().unwrap().len(), 1);
    assert_eq!(fallback.recorded_requests().unwrap().len(), 1);
}

#[test]
fn non_retryable_provider_errors_stop_failover() {
    let registry = Arc::new(ProviderRegistry::new());
    let primary = provider("primary", "primary-model", 1, 1, true, true);
    let fallback = provider("fallback", "fallback-model", 1, 1, true, true);
    let last = provider("last", "last-model", 1, 1, true, true);
    primary
        .push_error(ProviderErrorKind::Timeout, "temporary timeout")
        .unwrap();
    fallback
        .push_error(ProviderErrorKind::Authentication, "invalid credential")
        .unwrap();
    last.push_response(response("last-model", "must not run"))
        .unwrap();
    register(
        &registry,
        &primary,
        "primary-model",
        profile(9_000, "1s", DataResidency::Remote),
    );
    register(
        &registry,
        &fallback,
        "fallback-model",
        profile(8_000, "1s", DataResidency::Remote),
    );
    register(
        &registry,
        &last,
        "last-model",
        profile(7_000, "1s", DataResidency::Remote),
    );
    let router = ModelRouter::new(registry);

    let error = ready(router.generate(auto_request(&[OptimizationObjective::Quality])))
        .expect_err("authentication must stop failover");
    let RouterError::ProviderRejected {
        error,
        prior_failures,
    } = error
    else {
        panic!("expected terminal provider rejection");
    };
    assert_eq!(error.provider().as_str(), "fallback");
    assert_eq!(prior_failures.len(), 1);
    assert!(last.recorded_requests().unwrap().is_empty());
}

#[test]
fn streaming_uses_the_same_selection_and_auditable_route() {
    let registry = Arc::new(ProviderRegistry::new());
    let adapter = provider("stream", "stream-model", 1, 1, true, true);
    adapter
        .push_response(response("stream-model", "chunk"))
        .unwrap();
    register(
        &registry,
        &adapter,
        "stream-model",
        profile(8_000, "1s", DataResidency::Confidential),
    );
    let router = ModelRouter::new(registry);
    let mut stream = ready(router.stream(auto_request(&[OptimizationObjective::Latency]))).unwrap();

    assert_eq!(stream.decision().provider().as_str(), "stream");
    assert!(matches!(
        ready(stream.next()).unwrap(),
        Some(StreamEvent::Started { .. })
    ));
}
