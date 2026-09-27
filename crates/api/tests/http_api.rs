use agentkube_agents::{
    AgentDefinition, AgentDeployment, AgentDeploymentSpec, AgentRole, AgentSpec, Instructions,
    ModelName, ModelPolicy, ProviderName, ReplicaCount,
};
use agentkube_api::{ApiState, router};
use agentkube_core::{HumanDuration, Metadata, NodeId, TaskId};
use agentkube_queue::{
    EnqueueRequest, InMemoryTaskQueue, LeaseId, QueueError, QueueFuture, QueueStats, TaskLease,
    TaskQueue,
};
use agentkube_storage::{InMemoryResourceRepository, ResourceRepository};
use agentkube_tasks::{AgentTask, Objective, TaskSpec};
use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct FailingQueue;

impl TaskQueue for FailingQueue {
    fn enqueue<'a>(&'a self, _request: EnqueueRequest) -> QueueFuture<'a, ()> {
        Box::pin(async { Err(QueueError::Unavailable("injected queue failure")) })
    }

    fn lease<'a>(
        &'a self,
        _consumer: NodeId,
        _duration: HumanDuration,
    ) -> QueueFuture<'a, Option<TaskLease>> {
        Box::pin(async { Ok(None) })
    }

    fn acknowledge<'a>(&'a self, _lease_id: LeaseId, _consumer: NodeId) -> QueueFuture<'a, TaskId> {
        Box::pin(async { Err(QueueError::Unavailable("not supported")) })
    }

    fn release<'a>(
        &'a self,
        _lease_id: LeaseId,
        _consumer: NodeId,
        _delay: Option<HumanDuration>,
    ) -> QueueFuture<'a, TaskId> {
        Box::pin(async { Err(QueueError::Unavailable("not supported")) })
    }

    fn extend<'a>(
        &'a self,
        _lease_id: LeaseId,
        _consumer: NodeId,
        _duration: HumanDuration,
    ) -> QueueFuture<'a, TaskLease> {
        Box::pin(async { Err(QueueError::Unavailable("not supported")) })
    }

    fn stats<'a>(&'a self) -> QueueFuture<'a, QueueStats> {
        Box::pin(async { Ok(QueueStats::default()) })
    }
}

struct Fixture {
    app: Router,
    agents: Arc<InMemoryResourceRepository<AgentDefinition>>,
    deployments: Arc<InMemoryResourceRepository<AgentDeployment>>,
    tasks: Arc<InMemoryResourceRepository<AgentTask>>,
    queue: Arc<InMemoryTaskQueue>,
}

fn fixture(max_body_bytes: usize) -> Fixture {
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
    Fixture {
        app: router(state, max_body_bytes),
        agents,
        deployments,
        tasks,
        queue,
    }
}

fn agent_document(name: &str) -> Value {
    serde_json::to_value(
        AgentDefinition::new(
            Metadata::new(name).unwrap(),
            AgentSpec::new(
                AgentRole::new("developer").unwrap(),
                ModelPolicy::fixed(
                    ProviderName::new("openai").unwrap(),
                    ModelName::new("gpt-5").unwrap(),
                ),
                Instructions::new("Build reliable software.").unwrap(),
            ),
        )
        .into_document(),
    )
    .unwrap()
}

fn deployment_document(name: &str, replicas: u32) -> Value {
    serde_json::to_value(
        AgentDeployment::new(
            Metadata::new(name).unwrap(),
            AgentDeploymentSpec::new(
                ReplicaCount::new(replicas).unwrap(),
                AgentSpec::new(
                    AgentRole::new("developer").unwrap(),
                    ModelPolicy::fixed(
                        ProviderName::new("openai").unwrap(),
                        ModelName::new("gpt-5").unwrap(),
                    ),
                    Instructions::new("Serve deployment work.").unwrap(),
                ),
            ),
        )
        .unwrap()
        .into_document(),
    )
    .unwrap()
}

fn task_document(name: &str) -> Value {
    serde_json::to_value(
        AgentTask::new(
            Metadata::new(name).unwrap(),
            TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap()),
        )
        .into_document(),
    )
    .unwrap()
}

async fn request(app: &Router, method: &str, uri: &str, body: Option<&Value>) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    let request_body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(value).unwrap())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(request_body).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_readiness_and_unknown_routes_have_stable_responses() {
    let fixture = fixture(1024 * 1024);
    let health = request(&fixture.app, "GET", "/healthz", None).await;
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(response_json(health).await["status"], "ok");

    let ready = request(&fixture.app, "GET", "/readyz", None).await;
    assert_eq!(ready.status(), StatusCode::OK);

    let missing = request(&fixture.app, "GET", "/v1/unknown", None).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let error = response_json(missing).await;
    assert_eq!(error["code"], 404);
    assert_eq!(error["reason"], "NOT_FOUND");
}

#[tokio::test]
async fn agent_crud_enforces_optimistic_concurrency_and_server_owned_status() {
    let fixture = fixture(1024 * 1024);
    let document = agent_document("backend-agent");
    let created = request(&fixture.app, "POST", "/v1/agents", Some(&document)).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;

    let fetched = request(&fixture.app, "GET", "/v1/agents/backend-agent", None).await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(response_json(fetched).await, created);

    let mut replacement = created.clone();
    replacement["spec"]["instructions"] = json!("Build even more reliable software.");
    replacement["status"]["phase"] = json!("READY");
    let replaced = request(
        &fixture.app,
        "PUT",
        "/v1/agents/backend-agent",
        Some(&replacement),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = response_json(replaced).await;
    assert_eq!(replaced["metadata"]["resourceVersion"], 2);
    assert_eq!(replaced["status"]["phase"], "PENDING");

    let stale = request(
        &fixture.app,
        "PUT",
        "/v1/agents/backend-agent",
        Some(&replacement),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let deleted = request(&fixture.app, "DELETE", "/v1/agents/backend-agent", None).await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(fixture.agents.list(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_resources_and_path_identity_mismatches_are_structured_errors() {
    let fixture = fixture(1024 * 1024);
    let document = agent_document("planner");
    assert_eq!(
        request(&fixture.app, "POST", "/v1/agents", Some(&document))
            .await
            .status(),
        StatusCode::CREATED
    );
    let duplicate = request(&fixture.app, "POST", "/v1/agents", Some(&document)).await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(duplicate).await["reason"], "CONFLICT");

    let mismatch = request(
        &fixture.app,
        "PUT",
        "/v1/agents/different-name",
        Some(&document),
    )
    .await;
    assert_eq!(mismatch.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response_json(mismatch).await["reason"], "VALIDATION_FAILED");
}

#[tokio::test]
async fn agent_lists_are_deterministic_and_use_bounded_continuation_tokens() {
    let fixture = fixture(1024 * 1024);
    for name in ["charlie", "alpha", "bravo"] {
        let response = request(
            &fixture.app,
            "POST",
            "/v1/agents",
            Some(&agent_document(name)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let first = request(&fixture.app, "GET", "/v1/agents?limit=2", None).await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = response_json(first).await;
    assert_eq!(first["items"][0]["metadata"]["name"], "alpha");
    assert_eq!(first["items"][1]["metadata"]["name"], "bravo");
    assert_eq!(first["metadata"]["remainingItemCount"], 1);
    assert_eq!(first["metadata"]["continue"], "default/bravo");

    let second = request(
        &fixture.app,
        "GET",
        "/v1/agents?limit=2&continue=default%2Fbravo",
        None,
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second = response_json(second).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["metadata"]["name"], "charlie");

    let excessive = request(&fixture.app, "GET", "/v1/agents?limit=201", None).await;
    assert_eq!(excessive.status(), StatusCode::BAD_REQUEST);

    let invalid = request(
        &fixture.app,
        "GET",
        "/v1/agents?continue=default%2Fmissing",
        None,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deployment_create_replace_list_and_delete_preserve_resource_versions() {
    let fixture = fixture(1024 * 1024);
    let document = deployment_document("workers", 2);
    let created = request(&fixture.app, "POST", "/v1/deployments", Some(&document)).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let mut created = response_json(created).await;
    created["spec"]["replicas"] = json!(4);

    let replaced = request(
        &fixture.app,
        "PUT",
        "/v1/deployments/workers",
        Some(&created),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = response_json(replaced).await;
    assert_eq!(replaced["metadata"]["resourceVersion"], 2);
    assert_eq!(replaced["spec"]["replicas"], 4);
    assert_eq!(replaced["status"]["desiredReplicas"], 4);

    let listed = request(&fixture.app, "GET", "/v1/deployments", None).await;
    assert_eq!(
        response_json(listed).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fixture.deployments.list(None).await.unwrap().len(), 1);

    let deleted = request(&fixture.app, "DELETE", "/v1/deployments/workers", None).await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn creating_a_task_persists_queued_state_and_dispatches_exactly_once() {
    let fixture = fixture(1024 * 1024);
    let created = request(
        &fixture.app,
        "POST",
        "/v1/tasks",
        Some(&task_document("review-code")),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    assert_eq!(created["status"]["state"], "QUEUED");
    assert_eq!(fixture.tasks.list(None).await.unwrap().len(), 1);
    let stats = fixture.queue.stats().await.unwrap();
    assert_eq!((stats.ready(), stats.delayed(), stats.leased()), (1, 0, 0));

    let fetched = request(&fixture.app, "GET", "/v1/tasks/review-code", None).await;
    assert_eq!(response_json(fetched).await["status"]["state"], "QUEUED");
    let deletion = request(&fixture.app, "DELETE", "/v1/tasks/review-code", None).await;
    assert_eq!(deletion.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn terminal_tasks_can_be_deleted_safely() {
    let fixture = fixture(1024 * 1024);
    let mut task = AgentTask::new(
        Metadata::new("cancelled-task").unwrap(),
        TaskSpec::new(Objective::new("No longer needed").unwrap()),
    );
    task.cancel().unwrap();
    fixture.tasks.create(task).await.unwrap();

    let deleted = request(&fixture.app, "DELETE", "/v1/tasks/cancelled-task", None).await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(fixture.tasks.list(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn task_creation_rolls_back_persistence_when_dispatch_fails() {
    let agents = Arc::new(InMemoryResourceRepository::new());
    let deployments = Arc::new(InMemoryResourceRepository::new());
    let tasks = Arc::new(InMemoryResourceRepository::new());
    let state = ApiState::new(agents, deployments, tasks.clone(), Arc::new(FailingQueue));
    let app = router(state, 1024 * 1024);

    let response = request(
        &app,
        "POST",
        "/v1/tasks",
        Some(&task_document("rollback-task")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response_json(response).await["code"], 503);
    assert!(tasks.list(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn malformed_json_and_oversized_payloads_do_not_escape_error_handling() {
    let fixture = fixture(128);
    let malformed = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(malformed).await["reason"], "BAD_REQUEST");

    let oversized = request(
        &fixture.app,
        "POST",
        "/v1/agents",
        Some(&agent_document("oversized-agent")),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(oversized).await["reason"], "BAD_REQUEST");
}
