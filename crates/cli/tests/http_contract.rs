//! HTTP contract tests against the real `agentkube-api` router.
//!
//! These tests validate the wire contract exercised by `akctl`: health,
//! apply create/update, scale with conflicts, task QUEUED state, pagination,
//! structured errors, retries, deletes, and size limits.

use agentkube_agents::{
    AgentDefinition, AgentDeployment, AgentDeploymentSpec, AgentRole, AgentSpec, Instructions,
    ModelName, ModelPolicy, ProviderName, ReplicaCount,
};
use agentkube_api::{ApiState, router, serve};
use agentkube_cli::client::{ApiClient, HealthResponse};
use agentkube_core::{Metadata, Namespace};
use agentkube_queue::InMemoryTaskQueue;
use agentkube_storage::{InMemoryResourceRepository, ResourceRepository};
use agentkube_tasks::{AgentTask, Objective, TaskSpec};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;

struct TestServer {
    base_url: String,
    tasks: Arc<InMemoryResourceRepository<AgentTask>>,
}

async fn spawn_server() -> TestServer {
    spawn_server_with_body_limit(2 * 1024 * 1024).await
}

async fn spawn_server_with_body_limit(max_body_bytes: usize) -> TestServer {
    let agents = Arc::new(InMemoryResourceRepository::new());
    let deployments = Arc::new(InMemoryResourceRepository::new());
    let tasks = Arc::new(InMemoryResourceRepository::new());
    let queue = Arc::new(InMemoryTaskQueue::new());
    let state = ApiState::new(
        agents.clone(),
        deployments.clone(),
        tasks.clone(),
        queue.clone(),
    );
    let app = router(state, max_body_bytes);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve(listener, app).await;
    });
    TestServer {
        base_url: format!("http://{addr}"),
        tasks,
    }
}

fn test_client(base_url: &str) -> ApiClient {
    ApiClient::new(base_url.to_owned(), Duration::from_secs(5), None, 0).unwrap()
}

fn agent_spec() -> AgentSpec {
    AgentSpec::new(
        AgentRole::new("developer").unwrap(),
        ModelPolicy::fixed(
            ProviderName::new("openai").unwrap(),
            ModelName::new("gpt-5").unwrap(),
        ),
        Instructions::new("Build reliable software.").unwrap(),
    )
}

fn agent_document(name: &str) -> agentkube_agents::AgentDocument {
    AgentDefinition::new(Metadata::new(name).unwrap(), agent_spec()).into_document()
}

fn deployment_document(name: &str, replicas: u32) -> agentkube_agents::AgentDeploymentDocument {
    AgentDeployment::new(
        Metadata::new(name).unwrap(),
        AgentDeploymentSpec::new(ReplicaCount::new(replicas).unwrap(), agent_spec()),
    )
    .unwrap()
    .into_document()
}

fn task_document(name: &str) -> agentkube_tasks::TaskDocument {
    AgentTask::new(
        Metadata::new(name).unwrap(),
        TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap()),
    )
    .into_document()
}

#[tokio::test]
async fn health_and_readiness_use_real_endpoints() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    let health: HealthResponse = client.health().await.unwrap();
    assert_eq!(health.status, "ok");
    let ready: HealthResponse = client.ready().await.unwrap();
    assert_eq!(ready.status, "ok");
}

#[tokio::test]
async fn agent_apply_create_and_update_preserve_uid_and_bump_version() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    let created = client
        .create_agent(&agent_document("backend-agent"))
        .await
        .unwrap();
    let uid = created.metadata().uid();
    assert_eq!(created.metadata().resource_version().get(), 1);

    // Update with server UID and current version succeeds and bumps to 2.
    // Modify via JSON to avoid domain mutators: change instructions through a fresh spec.
    let mut value = serde_json::to_value(&created).unwrap();
    value["spec"]["instructions"] = serde_json::json!("Build even more reliable software.");
    let updated_doc: agentkube_agents::AgentDocument = serde_json::from_value(value).unwrap();
    let replaced = client
        .replace_agent("backend-agent", &updated_doc)
        .await
        .unwrap();
    assert_eq!(replaced.metadata().uid(), uid);
    assert_eq!(replaced.metadata().resource_version().get(), 2);

    // Stale replacement surfaces 409 instead of silent overwrite.
    let stale_result = client.replace_agent("backend-agent", &updated_doc).await;
    match stale_result {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 409),
        other => panic!("expected 409 conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn deployment_apply_and_scale_enforce_optimistic_concurrency() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    let created = client
        .create_deployment(&deployment_document("workers", 2))
        .await
        .unwrap();
    assert_eq!(created.spec().replicas().get(), 2);

    // Scale by GET, change only replicas, PUT with current version.
    let current = client.get_deployment("workers").await.unwrap();
    let mut deployment = AgentDeployment::from_document(current).unwrap();
    deployment.spec_mut().scale(ReplicaCount::new(4).unwrap());
    let scaled = client
        .replace_deployment("workers", &deployment.into_document())
        .await
        .unwrap();
    assert_eq!(scaled.spec().replicas().get(), 4);
    assert_eq!(scaled.metadata().resource_version().get(), 2);

    // Stale deployment replacement surfaces 409.
    match client.replace_deployment("workers", &created).await {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 409),
        other => panic!("expected 409 for stale deployment, got {other:?}"),
    }

    // Replica range is enforced client-side through ReplicaCount.
    assert!(ReplicaCount::new(10_001).is_err());
    assert!(ReplicaCount::new(10_000).is_ok());
}

#[tokio::test]
async fn bearer_tokens_are_sent_without_leaking_into_diagnostics() {
    let server = spawn_server().await;
    let client = ApiClient::new(
        server.base_url.clone(),
        Duration::from_secs(5),
        Some("super-secret-token".to_owned()),
        0,
    )
    .unwrap();
    // The test server ignores auth but the request must still succeed,
    // proving the Authorization header does not break the contract.
    let health = client.health().await.unwrap();
    assert_eq!(health.status, "ok");

    // Diagnostics never include the token value.
    let missing = client.get_agent("missing-token-check").await.unwrap_err();
    assert!(!missing.to_string().contains("super-secret-token"));
}

#[tokio::test]
async fn status_summary_aggregates_agents_deployments_and_tasks() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    client.create_agent(&agent_document("web")).await.unwrap();
    client
        .create_deployment(&deployment_document("web", 2))
        .await
        .unwrap();
    client.create_task(&task_document("job")).await.unwrap();

    let agents = client.list_all_agents(None, None).await.unwrap();
    let deployments = client.list_all_deployments(None, None).await.unwrap();
    let tasks = client.list_all_tasks(None, None).await.unwrap();
    let summary = agentkube_cli::output::summarize_status(&agents, &deployments, &tasks);
    assert_eq!(summary.agents, 1);
    assert_eq!(summary.deployments, 1);
    assert_eq!(summary.desired_replicas, 2);
    assert_eq!(summary.ready_replicas, 0);
    assert_eq!(summary.tasks_by_state.get("QUEUED"), Some(&1));

    let health = client.health().await.unwrap();
    let ready = client.ready().await.unwrap();
    let rendered = agentkube_cli::output::render_status_table(
        &server.base_url,
        &health,
        &ready,
        &agents,
        &deployments,
        &tasks,
        true,
    );
    for needle in [
        "Server:",
        "AGENTS (1)",
        "DEPLOYMENTS (1, desired 2, ready 0)",
        "TASKS (1, QUEUED 1)",
        "web",
        "job",
    ] {
        assert!(
            rendered.contains(needle),
            "missing {needle:?} in:\n{rendered}"
        );
    }
}

#[tokio::test]
async fn task_apply_returns_queued_state() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    let created = client
        .create_task(&task_document("review-code"))
        .await
        .unwrap();
    let status = created.status().expect("task has status");
    assert_eq!(status.state(), agentkube_tasks::TaskState::Queued);
    assert_eq!(status.attempts_started(), 0);
    assert!(status.assigned_agent().is_none());
}

#[tokio::test]
async fn multi_page_lists_are_combined_in_order() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    for name in ["charlie", "alpha", "bravo"] {
        client.create_agent(&agent_document(name)).await.unwrap();
    }
    // Page size 2 forces at least two pages; client must follow tokens.
    let all = client.list_all_agents(Some(2), None).await.unwrap();
    let names: Vec<String> = all
        .iter()
        .map(|doc| doc.metadata().name().as_str().to_owned())
        .collect();
    assert_eq!(names, vec!["alpha", "bravo", "charlie"]);

    // Manual pagination from a token still follows to the end.
    let from_bravo = client
        .list_all_agents(Some(2), Some("default/bravo"))
        .await
        .unwrap();
    assert_eq!(from_bravo.len(), 1);
    assert_eq!(from_bravo[0].metadata().name().as_str(), "charlie");
}

#[tokio::test]
async fn repeated_continuation_tokens_are_rejected() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let app = axum::Router::new().route(
        "/v1/agents",
        axum::routing::get(move || {
            let hits = hits_clone.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "apiVersion": "agentkube.ai/v1",
                    "kind": "AgentList",
                    "metadata": {"continue": "stuck-token"},
                    "items": [],
                }))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = test_client(&format!("http://{addr}"));
    let result = client.list_all_agents(Some(10), None).await;
    match result {
        Err(agentkube_cli::error::CliError::Transport { message, .. }) => {
            assert!(message.contains("repeated continuation token"), "{message}");
        }
        other => panic!("expected repeated-token protection, got {other:?}"),
    }
    assert!(hits.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn structured_errors_cover_404_409_422() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    // 404 for missing agent.
    match client.get_agent("missing").await {
        Err(agentkube_cli::error::CliError::Api { status, error, .. }) => {
            assert_eq!(status, 404);
            assert_eq!(error.code().get(), 404);
        }
        other => panic!("expected 404, got {other:?}"),
    }

    // 409 for duplicate create.
    client.create_agent(&agent_document("dup")).await.unwrap();
    match client.create_agent(&agent_document("dup")).await {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 409),
        other => panic!("expected 409, got {other:?}"),
    }

    // 422 for non-default namespace.
    let bad = AgentDefinition::new(
        Metadata::in_namespace("bad-ns", Namespace::new("other").unwrap()).unwrap(),
        agent_spec(),
    )
    .into_document();
    match client.create_agent(&bad).await {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 422),
        other => panic!("expected 422, got {other:?}"),
    }
}

#[tokio::test]
async fn service_unavailable_maps_to_retryable_runtime_error() {
    use agentkube_core::{HumanDuration, NodeId, TaskId};
    use agentkube_queue::{EnqueueRequest, QueueError};
    use agentkube_queue::{
        InMemoryTaskQueue, LeaseId, QueueFuture, QueueStats, TaskLease, TaskQueue,
    };

    struct FailingQueue;
    impl TaskQueue for FailingQueue {
        fn enqueue<'a>(&'a self, _request: EnqueueRequest) -> QueueFuture<'a, ()> {
            Box::pin(async { Err(QueueError::Unavailable("injected failure")) })
        }
        fn lease<'a>(
            &'a self,
            _consumer: NodeId,
            _duration: HumanDuration,
        ) -> QueueFuture<'a, Option<TaskLease>> {
            Box::pin(async { Ok(None) })
        }
        fn acknowledge<'a>(
            &'a self,
            _lease_id: LeaseId,
            _consumer: NodeId,
        ) -> QueueFuture<'a, TaskId> {
            Box::pin(async { Err(QueueError::Unavailable("no")) })
        }
        fn release<'a>(
            &'a self,
            _lease_id: LeaseId,
            _consumer: NodeId,
            _delay: Option<HumanDuration>,
        ) -> QueueFuture<'a, TaskId> {
            Box::pin(async { Err(QueueError::Unavailable("no")) })
        }
        fn extend<'a>(
            &'a self,
            _lease_id: LeaseId,
            _consumer: NodeId,
            _duration: HumanDuration,
        ) -> QueueFuture<'a, TaskLease> {
            Box::pin(async { Err(QueueError::Unavailable("no")) })
        }
        fn stats<'a>(&'a self) -> QueueFuture<'a, QueueStats> {
            Box::pin(async { Ok(QueueStats::default()) })
        }
    }

    let agents = Arc::new(InMemoryResourceRepository::new());
    let deployments = Arc::new(InMemoryResourceRepository::<AgentDeployment>::new());
    let tasks = Arc::new(InMemoryResourceRepository::new());
    let state = ApiState::new(agents, deployments, tasks, Arc::new(FailingQueue));
    let app = router(state, 2 * 1024 * 1024);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve(listener, app).await;
    });
    let client = test_client(&format!("http://{addr}"));
    match client.create_task(&task_document("rollback")).await {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 503),
        other => panic!("expected 503, got {other:?}"),
    }
    let _ = InMemoryTaskQueue::new();
}

#[tokio::test]
async fn timeout_and_unreachable_servers_are_runtime_failures() {
    // Unroutable connection-refused port fails fast with exit code 1.
    let client = ApiClient::new(
        "http://127.0.0.1:1".to_owned(),
        Duration::from_millis(500),
        None,
        0,
    )
    .unwrap();
    match client.health().await {
        Err(agentkube_cli::error::CliError::Transport { .. }) => {}
        other => panic!("expected transport failure, got {other:?}"),
    }
    // Exit-code mapping is stable.
    let error = agentkube_cli::error::CliError::transport("check health", "down");
    assert_eq!(error.exit_code(), 1);
}

#[tokio::test]
async fn mutating_requests_are_not_retried_while_gets_are() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // POST returning 503 must be attempted exactly once.
    let posts = Arc::new(AtomicUsize::new(0));
    let posts_clone = posts.clone();
    let app = axum::Router::new().route(
        "/v1/agents",
        axum::routing::post(move || {
            let posts = posts_clone.clone();
            async move {
                posts.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(serde_json::json!({
                        "code": 503,
                        "reason": "INTERNAL",
                        "message": "busy",
                    })),
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = ApiClient::new(format!("http://{addr}"), Duration::from_secs(2), None, 0).unwrap();
    let _ = client.create_agent(&agent_document("no-retry")).await;
    assert_eq!(posts.load(Ordering::SeqCst), 1, "POST must not be retried");

    // GET returning 503 twice then success must be retried.
    let gets = Arc::new(AtomicUsize::new(0));
    let gets_clone = gets.clone();
    let app = axum::Router::new().route(
        "/healthz",
        axum::routing::get(move || {
            let gets = gets_clone.clone();
            async move {
                let attempt = gets.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(serde_json::json!({
                            "code": 503,
                            "reason": "INTERNAL",
                            "message": "busy",
                        })),
                    )
                        .into_response()
                } else {
                    axum::Json(serde_json::json!({
                        "status": "ok",
                        "version": "0.1.0",
                    }))
                    .into_response()
                }
            }
        }),
    );
    use axum::response::IntoResponse;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = ApiClient::new(format!("http://{addr}"), Duration::from_secs(5), None, 0).unwrap();
    let health = client.health().await.unwrap();
    assert_eq!(health.status, "ok");
    assert!(gets.load(Ordering::SeqCst) >= 3, "GET must be retried");
}

#[tokio::test]
async fn delete_behaviour_includes_terminal_task_conflicts() {
    let server = spawn_server().await;
    let client = test_client(&server.base_url);

    client.create_agent(&agent_document("gone")).await.unwrap();
    client.delete_agent("gone").await.unwrap();
    match client.get_agent("gone").await {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 404),
        other => panic!("expected 404 after delete, got {other:?}"),
    }

    // Non-terminal (QUEUED) tasks cannot be deleted.
    client.create_task(&task_document("queued")).await.unwrap();
    match client.delete_task("queued").await {
        Err(agentkube_cli::error::CliError::Api { status, .. }) => assert_eq!(status, 409),
        other => panic!("expected 409 for queued task, got {other:?}"),
    }

    // Terminal tasks can be deleted.
    let mut terminal = AgentTask::new(
        Metadata::new("done").unwrap(),
        TaskSpec::new(Objective::new("done").unwrap()),
    );
    terminal.cancel().unwrap();
    server.tasks.create(terminal).await.unwrap();
    client.delete_task("done").await.unwrap();
}

#[tokio::test]
async fn oversized_responses_are_discarded_without_dumping_bodies() {
    let big = "x".repeat(3 * 1024 * 1024);
    let app = axum::Router::new().route(
        "/healthz",
        axum::routing::get(move || {
            let big = big.clone();
            async move {
                axum::Json(serde_json::json!({
                    "status": big,
                    "version": "0.1.0",
                }))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = test_client(&format!("http://{addr}"));
    match client.health().await {
        Err(agentkube_cli::error::CliError::Transport { message, .. }) => {
            assert!(message.contains("ceiling"), "{message}");
            assert!(
                message.len() < 3 * 1024 * 1024,
                "must not dump oversized body"
            );
        }
        other => panic!("expected oversized rejection, got {other:?}"),
    }
}
