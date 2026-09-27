use agentkube_agents::{ModelName, ProviderName, ToolName};
use agentkube_core::HumanDuration;
use agentkube_providers::{
    ChatMessage, FinishReason, GenerationRequest, GenerationRequestError, GenerationResponse,
    GenerationUsage, MessageError, MessageText, ModelCapabilities, ModelProvider,
    ProviderCapabilities, ProviderErrorKind, ProviderHealth, ProviderHealthStatus,
    ScriptedProvider, StreamEvent, ToolCall, ToolCallId, ToolDefinition,
};
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
        Poll::Pending => panic!("scripted provider unexpectedly returned a pending future"),
    }
}

fn model() -> ModelName {
    ModelName::new("reasoner-v1").unwrap()
}

fn capabilities(tools: bool, streaming: bool) -> ProviderCapabilities {
    ProviderCapabilities::new([(
        model(),
        ModelCapabilities::new(
            NonZeroU32::new(128_000).unwrap(),
            NonZeroU32::new(4_096).unwrap(),
            tools,
            streaming,
            1_000_000,
            2_000_000,
        ),
    )])
    .unwrap()
}

fn request(text: &str) -> GenerationRequest {
    GenerationRequest::new(
        model(),
        vec![ChatMessage::user(MessageText::new(text).unwrap())],
    )
    .unwrap()
}

fn response(text: &str) -> GenerationResponse {
    GenerationResponse::text(
        model(),
        MessageText::new(text).unwrap(),
        FinishReason::Stop,
        GenerationUsage::new(10, 4, 0),
    )
    .unwrap()
}

#[test]
fn messages_and_tool_contracts_enforce_portable_invariants() {
    assert!(matches!(
        MessageText::new("  "),
        Err(MessageError::EmptyText(_))
    ));
    assert!(matches!(
        ToolCall::new(
            ToolCallId::new("call-1").unwrap(),
            ToolName::new("search").unwrap(),
            json!(["not", "an", "object"]),
        ),
        Err(MessageError::ToolArgumentsNotObject)
    ));
    assert!(matches!(
        ToolDefinition::new(
            ToolName::new("search").unwrap(),
            "Search indexed content",
            json!(true),
        ),
        Err(MessageError::ToolSchemaNotObject)
    ));
}

#[test]
fn requests_reject_duplicate_tools_and_nonportable_temperature() {
    let tool = ToolDefinition::new(
        ToolName::new("search").unwrap(),
        "Search indexed content",
        json!({"type": "object"}),
    )
    .unwrap();

    assert!(matches!(
        request("hello").with_tools(vec![tool.clone(), tool]),
        Err(GenerationRequestError::DuplicateTool(_))
    ));
    assert_eq!(
        request("hello").with_temperature_milli(2_001),
        Err(GenerationRequestError::TemperatureOutOfRange(2_001))
    );
}

#[test]
fn response_and_catalog_invariants_support_local_models() {
    assert!(
        GenerationResponse::text(
            model(),
            MessageText::new("invalid finish").unwrap(),
            FinishReason::ToolCalls,
            GenerationUsage::default(),
        )
        .is_err()
    );

    let local = ModelCapabilities::new(
        NonZeroU32::new(8_192).unwrap(),
        NonZeroU32::new(1_024).unwrap(),
        false,
        true,
        0,
        0,
    );
    assert_eq!(local.input_price(), 0);
    assert_eq!(local.output_price(), 0);
}

#[test]
fn scripted_provider_generates_and_records_valid_requests() {
    let provider =
        ScriptedProvider::new(ProviderName::new("test").unwrap(), capabilities(true, true));
    provider.push_response(response("done")).unwrap();
    let request = request("perform the task");

    let generated = ready(provider.generate(request.clone())).unwrap();

    assert_eq!(generated.generated_text(), Some("done"));
    assert_eq!(generated.usage().total_tokens(), Some(14));
    assert_eq!(provider.recorded_requests().unwrap(), vec![request]);
}

#[test]
fn provider_validates_models_output_limits_and_tool_support() {
    let provider = ScriptedProvider::new(
        ProviderName::new("limited").unwrap(),
        capabilities(false, false),
    );
    let unknown = GenerationRequest::new(
        ModelName::new("unknown").unwrap(),
        vec![ChatMessage::user(MessageText::new("hello").unwrap())],
    )
    .unwrap();
    assert_eq!(
        ready(provider.generate(unknown)).unwrap_err().kind(),
        ProviderErrorKind::ModelNotFound
    );

    let oversized = request("hello").with_max_output_tokens(NonZeroU32::new(4_097).unwrap());
    assert_eq!(
        ready(provider.generate(oversized)).unwrap_err().kind(),
        ProviderErrorKind::InvalidRequest
    );

    let tool = ToolDefinition::new(
        ToolName::new("search").unwrap(),
        "Search",
        json!({"type": "object"}),
    )
    .unwrap();
    let with_tool = request("hello").with_tools(vec![tool]).unwrap();
    assert_eq!(
        ready(provider.generate(with_tool)).unwrap_err().kind(),
        ProviderErrorKind::InvalidRequest
    );
}

#[test]
fn unavailable_health_prevents_inference_with_a_retryable_error() {
    let provider = ScriptedProvider::new(
        ProviderName::new("offline").unwrap(),
        capabilities(true, true),
    );
    provider
        .set_health(ProviderHealth::new(
            ProviderHealthStatus::Unavailable,
            Some("maintenance".to_owned()),
        ))
        .unwrap();

    let error = ready(provider.generate(request("hello"))).unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::Unavailable);
    assert!(error.is_retryable());
    assert_eq!(error.message(), "maintenance");
}

#[test]
fn normalized_errors_expose_failover_semantics() {
    let provider = ScriptedProvider::new(
        ProviderName::new("limited").unwrap(),
        capabilities(true, true),
    );
    provider
        .push_error(
            ProviderErrorKind::RateLimited {
                retry_after: Some("5s".parse::<HumanDuration>().unwrap()),
            },
            "capacity exhausted",
        )
        .unwrap();

    let error = ready(provider.generate(request("hello"))).unwrap_err();
    assert!(error.is_retryable());
    assert_eq!(error.provider().as_str(), "limited");
}

#[test]
fn cost_estimation_uses_catalog_prices_and_output_ceiling() {
    let provider = ScriptedProvider::new(
        ProviderName::new("priced").unwrap(),
        capabilities(true, true),
    );
    let request = request("four").with_max_output_tokens(NonZeroU32::new(100).unwrap());

    let estimate = ready(provider.estimate_cost(&request)).unwrap();

    assert_eq!(estimate.input_micro_usd(), 1);
    assert_eq!(estimate.output_micro_usd(), 200);
    assert_eq!(estimate.total_micro_usd(), Some(201));
}

#[test]
fn streaming_emits_a_typed_and_terminal_event_sequence() {
    let provider = ScriptedProvider::new(
        ProviderName::new("streaming").unwrap(),
        capabilities(true, true),
    );
    provider.push_response(response("chunk")).unwrap();
    let mut stream = ready(provider.stream(request("hello"))).unwrap();
    let mut events = Vec::new();
    while let Some(event) = ready(stream.next()).unwrap() {
        events.push(event);
    }

    assert!(matches!(events[0], StreamEvent::Started { .. }));
    assert_eq!(events[1], StreamEvent::TextDelta("chunk".to_owned()));
    assert!(matches!(events[2], StreamEvent::Usage(_)));
    assert_eq!(events[3], StreamEvent::Finished(FinishReason::Stop));
    assert_eq!(events.len(), 4);
}

#[test]
fn concurrent_calls_consume_each_scripted_response_once() {
    let provider = Arc::new(ScriptedProvider::new(
        ProviderName::new("concurrent").unwrap(),
        capabilities(true, true),
    ));
    for index in 0..8 {
        provider
            .push_response(response(&format!("response-{index}")))
            .unwrap();
    }
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let provider = Arc::clone(&provider);
            std::thread::spawn(move || {
                ready(provider.generate(request(&format!("request-{index}")))).unwrap()
            })
        })
        .collect();
    let responses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();

    let mut texts: Vec<_> = responses
        .iter()
        .map(|response| response.generated_text().unwrap())
        .collect();
    texts.sort_unstable();
    texts.dedup();
    assert_eq!(texts.len(), 8);
    assert_eq!(provider.recorded_requests().unwrap().len(), 8);
}
