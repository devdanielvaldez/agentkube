//! Operator recovery, reconciliation, and end-to-end dispatch.
//!
//! The dispatch test executes a real queued task through the router and a
//! scripted provider: nothing is mocked at the HTTP layer because the scripted
//! adapter speaks the genuine provider contract.

use agentkube_agents::{
    AgentDefinition, AgentDeployment, AgentDeploymentSpec, AgentRole, AgentSpec, Instructions,
    ModelName, ModelPolicy, ProviderName, ReplicaCount,
};
use agentkube_api::NodeRegistry;
use agentkube_core::{AgentId, Metadata, NodeId, Resource};
use agentkube_operator::{Dispatcher, ProviderDescriptor, reconcile_once, recover};
use agentkube_providers::{
    FinishReason, GenerationResponse, GenerationUsage, MessageText, ModelCapabilities,
    ModelProvider, ScriptedProvider,
};
use agentkube_queue::InMemoryTaskQueue;
use agentkube_router::{
    DataResidency, ModelRouter, ModelRouting, ModelRoutingProfile, ProviderRegistry,
};
use agentkube_scheduler::{NodeLocality, Scheduler};
use agentkube_sqlite::SqliteStores;
use agentkube_tasks::{AgentTask, Objective, RetryPolicy, TaskFailureKind, TaskSpec, TaskState};
use agentkube_workers::{
    AgentRuntime, ExecutionClaim, RepositoryWorkerStateStore, RuntimeFuture, RuntimeHealth,
    RuntimeOutput, SingleTurnRuntime,
};
use std::{
    num::{NonZeroU16, NonZeroU32},
    sync::{
        Arc,
        atomic::{AtomicU16, AtomicUsize, Ordering},
    },
    time::Duration,
};

const PROVIDER: &str = "scripted";
const MODEL: &str = "test-model";

fn definition(name: &str) -> AgentDefinition {
    AgentDefinition::new(
        Metadata::new(name).unwrap(),
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new(PROVIDER).unwrap(),
                ModelName::new(MODEL).unwrap(),
            ),
            Instructions::new("Build reliable software.").unwrap(),
        ),
    )
}

fn queued_task(name: &str) -> AgentTask {
    let mut task = AgentTask::new(
        Metadata::new(name).unwrap(),
        TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap()),
    );
    task.enqueue().unwrap();
    task
}

fn stores_in(path: &std::path::Path) -> SqliteStores {
    SqliteStores::open(path.join("agentkube.db")).unwrap()
}

fn scripted_setup() -> (Arc<dyn AgentRuntime>, Vec<ProviderDescriptor>) {
    let capabilities = agentkube_providers::ProviderCapabilities::new(vec![(
        ModelName::new(MODEL).unwrap(),
        ModelCapabilities::new(
            NonZeroU32::new(4096).unwrap(),
            NonZeroU32::new(1024).unwrap(),
            false,
            false,
            0,
            0,
        ),
    )])
    .unwrap();
    let provider = Arc::new(ScriptedProvider::new(
        ProviderName::new(PROVIDER).unwrap(),
        capabilities,
    ));
    for _ in 0..8 {
        provider
            .push_response(
                GenerationResponse::text(
                    ModelName::new(MODEL).unwrap(),
                    MessageText::new("done by scripted model").unwrap(),
                    FinishReason::Stop,
                    GenerationUsage::new(4, 2, 0),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(
            Arc::clone(&provider) as Arc<dyn ModelProvider>,
            vec![(
                ModelName::new(MODEL).unwrap(),
                ModelRoutingProfile::new(5_000, "30s".parse().unwrap(), DataResidency::Local)
                    .unwrap(),
            )],
        )
        .unwrap();
    let runtime: Arc<dyn AgentRuntime> = Arc::new(SingleTurnRuntime::new(
        Arc::new(ModelRouter::new(registry)) as Arc<dyn ModelRouting>,
    ));
    let providers = vec![ProviderDescriptor::new(
        ProviderName::new(PROVIDER).unwrap(),
        NodeLocality::Local,
        true,
    )];
    (runtime, providers)
}

struct DelayedRuntime {
    inner: Arc<dyn AgentRuntime>,
    active: AtomicUsize,
    maximum: AtomicUsize,
}

impl DelayedRuntime {
    fn new(inner: Arc<dyn AgentRuntime>) -> Self {
        Self {
            inner,
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
        }
    }
}

impl AgentRuntime for DelayedRuntime {
    fn health<'a>(&'a self) -> RuntimeFuture<'a, RuntimeHealth> {
        self.inner.health()
    }

    fn execute<'a>(&'a self, claim: ExecutionClaim) -> RuntimeFuture<'a, RuntimeOutput> {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            let result = self.inner.execute(claim).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            result
        })
    }
}

fn dispatcher_for(
    stores: &SqliteStores,
    queue: Arc<dyn agentkube_queue::TaskQueue>,
    state: RepositoryWorkerStateStore,
    runtime: Arc<dyn AgentRuntime>,
    providers: Vec<ProviderDescriptor>,
    nodes: Arc<NodeRegistry>,
    active: Arc<AtomicU16>,
) -> Dispatcher {
    Dispatcher::new(
        stores.tasks(),
        stores.agents(),
        queue,
        state,
        runtime,
        Scheduler::default(),
        NodeId::new(),
        providers,
        4,
        "60s".parse().unwrap(),
        nodes,
        active,
    )
}

#[tokio::test]
async fn dispatch_executes_a_queued_task_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let stores = stores_in(dir.path());
    stores.agents().create(definition("backend")).await.unwrap();
    stores.tasks().create(queued_task("build")).await.unwrap();

    let queue: Arc<dyn agentkube_queue::TaskQueue> = Arc::new(InMemoryTaskQueue::new());
    // The API creation path enqueues; mirror it explicitly here.
    let queued = stores
        .tasks()
        .list(None)
        .await
        .unwrap()
        .into_iter()
        .find(|task| task.status().state() == TaskState::Queued)
        .unwrap();
    queue
        .enqueue(agentkube_queue::EnqueueRequest::from_task(&queued).unwrap())
        .await
        .unwrap();

    let state = RepositoryWorkerStateStore::new(stores.tasks());
    state.resync().await.unwrap();
    let (runtime, providers) = scripted_setup();
    let dispatcher = dispatcher_for(
        &stores,
        Arc::clone(&queue),
        state,
        runtime,
        providers,
        Arc::new(NodeRegistry::new()),
        Arc::new(AtomicU16::new(0)),
    );

    let summary = dispatcher.dispatch_once().await.unwrap();
    assert_eq!(summary.executed, 1, "{summary:?}");

    let persisted = stores.tasks().list(None).await.unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status().state(), TaskState::Completed);
    assert_eq!(
        persisted[0].status().result().unwrap().output(),
        "done by scripted model"
    );
    assert_eq!(queue.stats().await.unwrap().total(), 0);
}

#[tokio::test]
async fn recovery_replays_crash_states_into_a_fresh_queue() {
    let dir = tempfile::tempdir().unwrap();
    let stores = stores_in(dir.path());

    let mut retryable = AgentTask::new(
        Metadata::new("flaky").unwrap(),
        TaskSpec::new(Objective::new("Flaky work.").unwrap()).with_retry_policy(
            RetryPolicy::new(NonZeroU16::new(3).unwrap())
                .with_retry_on(TaskFailureKind::WorkerLost),
        ),
    );
    retryable.enqueue().unwrap();
    retryable.schedule(AgentId::new()).unwrap();
    retryable.start().unwrap();
    stores.tasks().create(retryable).await.unwrap();

    let mut scheduled = queued_task("waiting");
    scheduled.schedule(AgentId::new()).unwrap();
    stores.tasks().create(scheduled).await.unwrap();

    stores.tasks().create(queued_task("queued")).await.unwrap();

    let queue: Arc<dyn agentkube_queue::TaskQueue> = Arc::new(InMemoryTaskQueue::new());
    let summary = recover(&stores.tasks(), Arc::clone(&queue)).await.unwrap();

    assert_eq!(summary.failed, 1, "{summary:?}");
    assert_eq!(summary.retried, 1, "{summary:?}");
    assert_eq!(summary.cancelled, 1, "{summary:?}");
    assert_eq!(summary.requeued, 2, "{summary:?}");
    assert_eq!(summary.skipped, 0, "{summary:?}");
    assert_eq!(queue.stats().await.unwrap().ready(), 2);

    let states: Vec<(String, TaskState)> = stores
        .tasks()
        .list(None)
        .await
        .unwrap()
        .into_iter()
        .map(|task| {
            (
                task.metadata().name().as_str().to_owned(),
                task.status().state(),
            )
        })
        .collect();
    assert!(states.contains(&("flaky".to_owned(), TaskState::Queued)));
    assert!(states.contains(&("waiting".to_owned(), TaskState::Cancelled)));
    assert!(states.contains(&("queued".to_owned(), TaskState::Queued)));
}

#[tokio::test]
async fn reconcile_persists_agent_status_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let stores = stores_in(dir.path());
    stores.agents().create(definition("backend")).await.unwrap();

    let state = RepositoryWorkerStateStore::new(stores.tasks());
    let node_id = NodeId::new();
    let first = reconcile_once(&stores.agents(), &stores.deployments(), &state, node_id)
        .await
        .unwrap();
    assert_eq!(first.updated, vec!["backend".to_owned()]);

    let second = reconcile_once(&stores.agents(), &stores.deployments(), &state, node_id)
        .await
        .unwrap();
    assert!(second.updated.is_empty(), "{second:?}");
}

#[tokio::test]
async fn deployment_reconcile_creates_scales_and_reports_live_replicas() {
    let dir = tempfile::tempdir().unwrap();
    let stores = stores_in(dir.path());
    let deployment = AgentDeployment::new(
        Metadata::new("coding-pool").unwrap(),
        AgentDeploymentSpec::new(
            ReplicaCount::new(3).unwrap(),
            definition("template").spec().clone(),
        ),
    )
    .unwrap();
    stores.deployments().create(deployment).await.unwrap();
    let state = RepositoryWorkerStateStore::new(stores.tasks());
    let node_id = NodeId::new();

    reconcile_once(&stores.agents(), &stores.deployments(), &state, node_id)
        .await
        .unwrap();
    let persisted = stores.deployments().list(None).await.unwrap().remove(0);
    assert_eq!(persisted.status().ready_replicas().get(), 3);
    assert_eq!(persisted.status().available_replicas().get(), 3);
    assert_eq!(state.instances().unwrap().len(), 3);

    let mut scaled_spec = persisted.spec().clone();
    scaled_spec.scale(ReplicaCount::new(1).unwrap());
    let scaled = AgentDeployment::new(persisted.metadata().clone(), scaled_spec).unwrap();
    stores.deployments().replace(scaled).await.unwrap();
    reconcile_once(&stores.agents(), &stores.deployments(), &state, node_id)
        .await
        .unwrap();
    let persisted = stores.deployments().list(None).await.unwrap().remove(0);
    assert_eq!(persisted.status().ready_replicas().get(), 1);
    assert_eq!(state.instances().unwrap().len(), 1);
}

#[tokio::test]
async fn dispatcher_runs_multiple_ready_replicas_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let stores = stores_in(dir.path());
    stores
        .deployments()
        .create(
            AgentDeployment::new(
                Metadata::new("parallel-pool").unwrap(),
                AgentDeploymentSpec::new(
                    ReplicaCount::new(2).unwrap(),
                    definition("template").spec().clone(),
                ),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let state = RepositoryWorkerStateStore::new(stores.tasks());
    let node_id = NodeId::new();
    reconcile_once(&stores.agents(), &stores.deployments(), &state, node_id)
        .await
        .unwrap();
    let queue: Arc<dyn agentkube_queue::TaskQueue> = Arc::new(InMemoryTaskQueue::new());
    for name in ["parallel-a", "parallel-b"] {
        let task = stores.tasks().create(queued_task(name)).await.unwrap();
        queue
            .enqueue(agentkube_queue::EnqueueRequest::from_task(&task).unwrap())
            .await
            .unwrap();
    }
    state.resync().await.unwrap();
    let (inner, providers) = scripted_setup();
    let runtime = Arc::new(DelayedRuntime::new(inner));
    let dispatcher = Dispatcher::new(
        stores.tasks(),
        stores.agents(),
        Arc::clone(&queue),
        state,
        Arc::clone(&runtime) as Arc<dyn AgentRuntime>,
        Scheduler::default(),
        node_id,
        providers,
        4,
        "20ms".parse().unwrap(),
        Arc::new(NodeRegistry::new()),
        Arc::new(AtomicU16::new(0)),
    );

    let summary = dispatcher.dispatch_once().await.unwrap();
    assert_eq!(summary.executed, 2, "{summary:?}");
    assert_eq!(runtime.maximum.load(Ordering::SeqCst), 2);
    assert!(
        stores
            .tasks()
            .list(None)
            .await
            .unwrap()
            .iter()
            .all(|task| task.status().state() == TaskState::Completed)
    );
}

#[test]
fn catalogs_register_keyed_providers_with_config_overrides() {
    use agentkube_config::{ConfigLoader, JsonSource};
    use agentkube_operator::build_catalogs;

    let entry = |name: &str, context: u32, max_output: u32| {
        serde_json::json!({
            "name": name,
            "contextWindowTokens": context,
            "maxOutputTokens": max_output,
            "supportsTools": true,
            "supportsStreaming": true,
            "inputPrice": 100,
            "outputPrice": 200,
        })
    };
    let document = serde_json::json!({
        "providers": {
            "anthropicApiKey": "sk-ant-test",
            "anthropicModels": [entry("claude-sonnet-4-5", 200_000, 32_000)],
            "geminiApiKey": "AIza-test",
            "geminiModels": [entry("gemini-2.5-flash", 1_048_576, 65_536)],
        },
    });
    let config = ConfigLoader::new()
        .with_source(JsonSource::new(
            "catalog-test",
            serde_json::to_string(&document).unwrap(),
        ))
        .load()
        .unwrap();

    let (catalogs, warnings) = build_catalogs(config.providers(), &[]).unwrap();

    assert!(warnings.is_empty(), "{warnings:?}");
    // Well-known table plus the config override (which wins by name).
    assert_eq!(catalogs.anthropic.len(), 2);
    let sonnet = catalogs
        .anthropic
        .iter()
        .find(|(name, _)| name.as_str() == "claude-sonnet-4-5")
        .expect("sonnet registered");
    assert_eq!(sonnet.1.max_output_tokens().get(), 32_000);
    assert_eq!(catalogs.gemini.len(), 2);
    // No OpenAI key: nothing registered there despite the well-known table.
    assert!(catalogs.openai.is_empty());
}
