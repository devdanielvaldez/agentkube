//! Contract tests for HTTP provider adapters against mock servers.
//!
//! Every test exercises real HTTP round-trips: no adapter behavior is
//! asserted without going through the wire format it claims to speak.

use agentkube_agents::{ModelName, ProviderName};
use agentkube_providers::{
    ChatMessage, GenerationRequest, MessageText, ModelCapabilities, ModelEventStream,
    ModelProvider, ProviderErrorKind, ProviderHealthStatus, StreamEvent,
};
use agentkube_providers_http::{
    AdapterError, AnthropicProvider, GeminiProvider, OllamaProvider, OpenAiProvider,
};
use axum::{
    Json, Router,
    routing::{get, post},
};
use std::num::NonZeroU32;

fn catalog() -> Vec<(ModelName, ModelCapabilities)> {
    vec![(
        ModelName::new("test-model").unwrap(),
        ModelCapabilities::new(
            NonZeroU32::new(4096).unwrap(),
            NonZeroU32::new(1024).unwrap(),
            false,
            true,
            100,
            200,
        ),
    )]
}

fn request() -> GenerationRequest {
    GenerationRequest::new(
        ModelName::new("test-model").unwrap(),
        vec![ChatMessage::user(MessageText::new("hello").unwrap())],
    )
    .unwrap()
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn collect(stream: &mut Box<dyn ModelEventStream>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await.unwrap() {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn ollama_generate_returns_text_and_reported_usage() {
    let base = serve(
        Router::new()
            .route(
                "/api/tags",
                get(|| async { Json(serde_json::json!({"models": []})) }),
            )
            .route(
                "/api/chat",
                post(|| async {
                    Json(serde_json::json!({
                        "model": "test-model",
                        "message": {"role": "assistant", "content": "hi there"},
                        "done": true,
                        "done_reason": "stop",
                        "prompt_eval_count": 10,
                        "eval_count": 5,
                    }))
                }),
            ),
    )
    .await;
    let provider = OllamaProvider::new(&base, catalog()).unwrap();

    assert_eq!(
        provider.health().await.unwrap().status(),
        ProviderHealthStatus::Healthy
    );
    let response = provider.generate(request()).await.unwrap();

    assert_eq!(response.generated_text(), Some("hi there"));
    assert_eq!(response.usage().input_tokens(), 10);
    assert_eq!(response.usage().output_tokens(), 5);
}

#[tokio::test]
async fn ollama_rejects_unknown_models_before_any_request() {
    let provider = OllamaProvider::new("http://127.0.0.1:1", catalog()).unwrap();
    let mut missing = request();
    missing = missing.for_model(ModelName::new("ghost").unwrap());

    match provider.generate(missing).await {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::ModelNotFound),
        Ok(_) => panic!("uncatalogued model must be rejected"),
    }
}

#[tokio::test]
async fn ollama_server_errors_map_to_normalized_kinds() {
    let base = serve(Router::new().route(
        "/api/chat",
        post(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "model not found"})),
            )
        }),
    ))
    .await;
    let provider = OllamaProvider::new(&base, catalog()).unwrap();

    match provider.generate(request()).await {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::ModelNotFound),
        Ok(_) => panic!("404 must surface as ModelNotFound"),
    }
}

#[tokio::test]
async fn ollama_unreachable_server_is_unavailable() {
    let provider = OllamaProvider::new("http://127.0.0.1:1", catalog()).unwrap();
    // Probing fails loudly so routers exclude the provider with a recorded
    // error instead of guessing from a default status.
    match provider.health().await {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::Unavailable),
        Ok(_) => panic!("unreachable server must fail probing"),
    }
}

#[tokio::test]
async fn ollama_stream_yields_ordered_events() {
    let base = serve(Router::new().route(
        "/api/chat",
        post(|| async {
            concat!(
                "{\"message\":{\"role\":\"assistant\",\"content\":\"hel\"},\"done\":false}\n",
                "{\"message\":{\"role\":\"assistant\",\"content\":\"lo\"},\"done\":false}\n",
                "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,",
                "\"done_reason\":\"stop\",\"prompt_eval_count\":4,\"eval_count\":2}\n",
            )
            .to_owned()
        }),
    ))
    .await;
    let _ = base;
    let provider = OllamaProvider::new(&base, catalog()).unwrap();

    let mut stream = provider.stream(request()).await.unwrap();
    let events = collect(&mut stream).await;
    let model = ModelName::new("test-model").unwrap();

    assert_eq!(
        events,
        vec![
            StreamEvent::Started {
                model: model.clone()
            },
            StreamEvent::TextDelta("hel".to_owned()),
            StreamEvent::TextDelta("lo".to_owned()),
            StreamEvent::Usage(agentkube_providers::GenerationUsage::new(4, 2, 0)),
            StreamEvent::Finished(agentkube_providers::FinishReason::Stop),
        ]
    );
}

#[tokio::test]
async fn openai_generate_maps_text_usage_and_tool_calls() {
    let base = serve(
        Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(serde_json::json!({"data": []})) }),
            )
            .route(
                "/v1/chat/completions",
                post(|| async {
                    Json(serde_json::json!({
                        "id": "chatcmpl-1",
                        "choices": [{
                            "message": {"role": "assistant", "content": "done!"},
                            "finish_reason": "stop",
                        }],
                        "usage": {
                            "prompt_tokens": 7,
                            "completion_tokens": 3,
                            "prompt_tokens_details": {"cached_tokens": 2},
                        },
                    }))
                }),
            ),
    )
    .await;
    let provider = OpenAiProvider::new(&base, "sk-test", catalog()).unwrap();

    assert_eq!(
        provider.health().await.unwrap().status(),
        ProviderHealthStatus::Healthy
    );
    let response = provider.generate(request()).await.unwrap();
    assert_eq!(response.generated_text(), Some("done!"));
    assert_eq!(response.usage().input_tokens(), 7);
    assert_eq!(response.usage().output_tokens(), 3);
    assert_eq!(response.usage().cached_input_tokens(), 2);
    assert!(response.generated_tool_calls().is_empty());
}

#[tokio::test]
async fn openai_tool_calls_are_preserved_with_identifiers() {
    let base = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "get-weather",
                                "arguments": "{\"city\":\"Oslo\"}",
                            },
                        }],
                    },
                    "finish_reason": "tool_calls",
                }],
                "usage": {"prompt_tokens": 9, "completion_tokens": 4},
            }))
        }),
    ))
    .await;
    let provider = OpenAiProvider::new(&base, "sk-test", catalog()).unwrap();

    let response = provider.generate(request()).await.unwrap();
    let calls = response.generated_tool_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id().as_str(), "call_1");
    assert_eq!(calls[0].name().as_str(), "get-weather");
}

#[tokio::test]
async fn openai_auth_and_rate_limit_errors_keep_their_kind() {
    let denied = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "bad key"})),
            )
        }),
    ))
    .await;
    let limited = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": "slow down"})),
            )
        }),
    ))
    .await;

    match OpenAiProvider::new(&denied, "bad", catalog())
        .unwrap()
        .generate(request())
        .await
    {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::Authentication),
        Ok(_) => panic!("401 must surface as Authentication"),
    }
    match OpenAiProvider::new(&limited, "sk-test", catalog())
        .unwrap()
        .generate(request())
        .await
    {
        Err(error) => assert_eq!(
            error.kind(),
            ProviderErrorKind::RateLimited { retry_after: None }
        ),
        Ok(_) => panic!("429 must surface as RateLimited"),
    }
}

#[tokio::test]
async fn openai_stream_reports_provider_usage() {
    let base = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":6,\"completion_tokens\":2}}\n\n",
                "data: [DONE]\n\n",
            )
            .to_owned()
        }),
    ))
    .await;
    let provider = OpenAiProvider::new(&base, "sk-test", catalog()).unwrap();

    let mut stream = provider.stream(request()).await.unwrap();
    let events = collect(&mut stream).await;
    let model = ModelName::new("test-model").unwrap();

    assert_eq!(
        events,
        vec![
            StreamEvent::Started { model },
            StreamEvent::TextDelta("hel".to_owned()),
            StreamEvent::TextDelta("lo".to_owned()),
            StreamEvent::Usage(agentkube_providers::GenerationUsage::new(6, 2, 0)),
            StreamEvent::Finished(agentkube_providers::FinishReason::Stop),
        ]
    );
}

#[tokio::test]
async fn construction_rejects_bad_urls_empty_keys_and_catalogs() {
    assert!(matches!(
        OllamaProvider::new("ftp://x", catalog()),
        Err(AdapterError::InvalidBaseUrl(_))
    ));
    assert!(matches!(
        OllamaProvider::new("http://x", Vec::new()),
        Err(AdapterError::InvalidCatalog(_))
    ));
    assert!(matches!(
        OpenAiProvider::new("http://x", "", catalog()),
        Err(AdapterError::EmptyApiKey)
    ));
    assert_eq!(
        OllamaProvider::PROVIDER_NAME,
        ProviderName::new("ollama").unwrap().as_str()
    );
    assert_eq!(
        OpenAiProvider::PROVIDER_NAME,
        ProviderName::new("openai").unwrap().as_str()
    );
}

#[tokio::test]
async fn estimates_follow_catalog_prices() {
    // "hello" is 5 bytes -> 2 estimated tokens at 100/million, plus the
    // 1024-token output ceiling at 200/million: (2*100+999999)/1e6 = 1 and
    // (1024*200+999999)/1e6 = 1.
    let provider = OllamaProvider::new("http://127.0.0.1:1", catalog()).unwrap();
    let estimate = provider.estimate_cost(&request()).await.unwrap();
    assert_eq!(estimate.input_micro_usd(), 1);
    assert_eq!(estimate.output_micro_usd(), 1);
}

#[tokio::test]
async fn anthropic_generate_returns_text_and_cached_usage() {
    let base = serve(
        Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(serde_json::json!({"data": []})) }),
            )
            .route(
                "/v1/messages",
                post(|| async {
                    Json(serde_json::json!({
                        "id": "msg_1",
                        "model": "test-model",
                        "role": "assistant",
                        "content": [{"type": "text", "text": "hello back"}],
                        "stop_reason": "end_turn",
                        "usage": {
                            "input_tokens": 8,
                            "output_tokens": 4,
                            "cache_read_input_tokens": 3,
                        },
                    }))
                }),
            ),
    )
    .await;
    let provider = AnthropicProvider::new(&base, "sk-ant-test", catalog()).unwrap();

    assert_eq!(
        provider.health().await.unwrap().status(),
        ProviderHealthStatus::Healthy
    );
    let response = provider.generate(request()).await.unwrap();
    assert_eq!(response.generated_text(), Some("hello back"));
    assert_eq!(response.usage().input_tokens(), 8);
    assert_eq!(response.usage().output_tokens(), 4);
    assert_eq!(response.usage().cached_input_tokens(), 3);
    assert!(response.generated_tool_calls().is_empty());
}

#[tokio::test]
async fn anthropic_tool_use_blocks_map_with_identifiers() {
    let base = serve(Router::new().route(
        "/v1/messages",
        post(|| async {
            Json(serde_json::json!({
                "id": "msg_2",
                "model": "test-model",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "get-weather",
                    "input": {"city": "Oslo"},
                }],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 9, "output_tokens": 5},
            }))
        }),
    ))
    .await;
    let provider = AnthropicProvider::new(&base, "sk-ant-test", catalog()).unwrap();

    let response = provider.generate(request()).await.unwrap();
    let calls = response.generated_tool_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id().as_str(), "toolu_1");
    assert_eq!(calls[0].name().as_str(), "get-weather");
}

#[tokio::test]
async fn anthropic_auth_and_rate_limit_errors_keep_their_kind() {
    let denied = serve(Router::new().route(
        "/v1/messages",
        post(|| async {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": {"type": "authentication_error"}})),
            )
        }),
    ))
    .await;
    let limited = serve(Router::new().route(
        "/v1/messages",
        post(|| async {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": {"type": "rate_limit_error"}})),
            )
        }),
    ))
    .await;

    match AnthropicProvider::new(&denied, "bad", catalog())
        .unwrap()
        .generate(request())
        .await
    {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::Authentication),
        Ok(_) => panic!("401 must surface as Authentication"),
    }
    match AnthropicProvider::new(&limited, "sk-ant-test", catalog())
        .unwrap()
        .generate(request())
        .await
    {
        Err(error) => {
            assert!(matches!(
                error.kind(),
                ProviderErrorKind::RateLimited { .. }
            ));
        }
        Ok(_) => panic!("429 must surface as RateLimited"),
    }
}

#[tokio::test]
async fn anthropic_stream_orders_events_and_merges_usage() {
    let base = serve(Router::new().route(
        "/v1/messages",
        post(|| async {
            concat!(
                "event: message_start\n",
                "data: {\"message\":{\"usage\":{\"input_tokens\":6,\"cache_read_input_tokens\":1}}}\n\n",
                "event: content_block_delta\n",
                "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
                "event: message_delta\n",
                "data: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                "event: message_stop\n",
                "data: {}\n\n",
            )
            .to_owned()
        }),
    ))
    .await;
    let provider = AnthropicProvider::new(&base, "sk-ant-test", catalog()).unwrap();

    let mut stream = provider.stream(request()).await.unwrap();
    let events = collect(&mut stream).await;
    let model = ModelName::new("test-model").unwrap();

    assert_eq!(
        events,
        vec![
            StreamEvent::Started { model },
            StreamEvent::TextDelta("hel".to_owned()),
            StreamEvent::TextDelta("lo".to_owned()),
            StreamEvent::Usage(agentkube_providers::GenerationUsage::new(6, 2, 1)),
            StreamEvent::Finished(agentkube_providers::FinishReason::Stop),
        ]
    );
}

#[tokio::test]
async fn gemini_generate_maps_text_usage_and_safety() {
    let base = serve(
        Router::new()
            .route(
                "/v1beta/models",
                get(|| async { Json(serde_json::json!({})) }),
            )
            .route(
                "/v1beta/models/test-model:generateContent",
                post(|| async {
                    Json(serde_json::json!({
                        "candidates": [{
                            "content": {"parts": [{"text": "hi from gemini"}]},
                            "finishReason": "STOP",
                        }],
                        "usageMetadata": {
                            "promptTokenCount": 5,
                            "candidatesTokenCount": 3,
                            "cachedContentTokenCount": 1,
                        },
                    }))
                }),
            ),
    )
    .await;
    let provider = GeminiProvider::new(&base, "AIza-test", catalog()).unwrap();

    assert_eq!(
        provider.health().await.unwrap().status(),
        ProviderHealthStatus::Healthy
    );
    let response = provider.generate(request()).await.unwrap();
    assert_eq!(response.generated_text(), Some("hi from gemini"));
    assert_eq!(response.usage().input_tokens(), 5);
    assert_eq!(response.usage().output_tokens(), 3);
    assert_eq!(response.usage().cached_input_tokens(), 1);
}

#[tokio::test]
async fn gemini_safety_filter_maps_to_filtered() {
    let base = serve(Router::new().route(
        "/v1beta/models/test-model:generateContent",
        post(|| async {
            Json(serde_json::json!({
                "candidates": [{
                    "content": {"parts": []},
                    "finishReason": "SAFETY",
                }],
                "usageMetadata": {
                    "promptTokenCount": 5,
                    "candidatesTokenCount": 0,
                },
            }))
        }),
    ))
    .await;
    let provider = GeminiProvider::new(&base, "AIza-test", catalog()).unwrap();

    let response = provider.generate(request()).await.unwrap();
    assert_eq!(
        response.finish_reason(),
        agentkube_providers::FinishReason::ContentFilter
    );
}

#[tokio::test]
async fn gemini_not_found_and_bad_request_keep_their_kind() {
    let missing = serve(Router::new().route(
        "/v1beta/models/test-model:generateContent",
        post(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": {"message": "not found"}})),
            )
        }),
    ))
    .await;
    let bad = serve(Router::new().route(
        "/v1beta/models/test-model:generateContent",
        post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": {"message": "bad request"}})),
            )
        }),
    ))
    .await;

    match GeminiProvider::new(&missing, "AIza-test", catalog())
        .unwrap()
        .generate(request())
        .await
    {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::ModelNotFound),
        Ok(_) => panic!("404 must surface as ModelNotFound"),
    }
    match GeminiProvider::new(&bad, "AIza-test", catalog())
        .unwrap()
        .generate(request())
        .await
    {
        Err(error) => assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest),
        Ok(_) => panic!("400 must surface as InvalidRequest"),
    }
}

#[tokio::test]
async fn gemini_stream_reports_provider_usage() {
    let base = serve(Router::new().route(
        "/v1beta/models/test-model:streamGenerateContent",
        post(|| async {
            concat!(
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hel\"}]}}]}\n\n",
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"}]}}]}\n\n",
                "data: {\"candidates\":[{\"finishReason\":\"STOP\"}],",
                "\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":2}}\n\n",
            )
            .to_owned()
        }),
    ))
    .await;
    let provider = GeminiProvider::new(&base, "AIza-test", catalog()).unwrap();

    let mut stream = provider.stream(request()).await.unwrap();
    let events = collect(&mut stream).await;
    let model = ModelName::new("test-model").unwrap();

    assert_eq!(
        events,
        vec![
            StreamEvent::Started { model },
            StreamEvent::TextDelta("hel".to_owned()),
            StreamEvent::TextDelta("lo".to_owned()),
            StreamEvent::Usage(agentkube_providers::GenerationUsage::new(4, 2, 0)),
            StreamEvent::Finished(agentkube_providers::FinishReason::Stop),
        ]
    );
}

#[tokio::test]
async fn construction_rejects_new_provider_misconfiguration() {
    assert!(matches!(
        AnthropicProvider::new("http://x", "", catalog()),
        Err(AdapterError::EmptyApiKey)
    ));
    assert!(matches!(
        GeminiProvider::new("not-a-url", "AIza-test", catalog()),
        Err(AdapterError::InvalidBaseUrl(_))
    ));
    assert_eq!(
        AnthropicProvider::PROVIDER_NAME,
        ProviderName::new("anthropic").unwrap().as_str()
    );
    assert_eq!(
        GeminiProvider::PROVIDER_NAME,
        ProviderName::new("gemini").unwrap().as_str()
    );
}
