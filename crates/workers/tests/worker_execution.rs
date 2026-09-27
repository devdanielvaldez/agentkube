use agentkube_agents::{
    AgentDefinition, AgentInstance, AgentInstanceState, AgentRole, AgentSpec, Instructions,
    ModelName, ModelPolicy, ProviderName,
};
use agentkube_core::{HumanDuration, Metadata, NodeId, Resource, ResourceUid};
use agentkube_providers::{
    FinishReason, GenerationResponse, GenerationUsage, MessageText, ModelCapabilities,
    ModelProvider, ProviderCapabilities, ProviderError, ProviderErrorKind, ScriptedProvider,
};
use agentkube_queue::{EnqueueRequest, InMemoryTaskQueue, TaskQueue};
use agentkube_router::{
    DataResidency, ModelRouter, ModelRouting, ModelRoutingProfile, ProviderRegistry, RouterError,
};
use agentkube_tasks::{
    AgentTask, BackoffPolicy, Objective, RetryPolicy, TaskFailureKind, TaskPriority, TaskSpec,
    TaskState,
};
use agentkube_workers::{
    AgentRuntime, DiscardReason, InMemoryWorkerStateStore, RuntimeError, RuntimeFuture,
    RuntimeHealth, RuntimeOutput, SingleTurnRuntime, Worker, WorkerError, WorkerRunOutcome,
    WorkerSettings,
};
use std::{
    future::Future,
    num::{NonZeroU16, NonZeroU32},
    sync::{Arc, Barrier},
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
        Poll::Pending => panic!("in-memory worker dependency unexpectedly returned pending"),
    }
}

fn duration(value: &str) -> HumanDuration {
    value.parse().unwrap()
}

fn definition(name: &str) -> AgentDefinition {
    AgentDefinition::new(
        Metadata::new(name).unwrap(),
        AgentSpec::new(
            AgentRole::new("engineer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("test-provider").unwrap(),
                ModelName::new("test-model").unwrap(),
            ),
            Instructions::new("Complete the task accurately.").unwrap(),
        ),
    )
}

fn ready_agent(definition_uid: ResourceUid, node_id: NodeId) -> AgentInstance {
    let mut agent = AgentInstance::new(definition_uid);
    agent.transition(AgentInstanceState::Scheduling).unwrap();
    agent.assign_node(node_id).unwrap();
    agent.transition(AgentInstanceState::Starting).unwrap();
    agent.transition(AgentInstanceState::Ready).unwrap();
    agent
}

fn queued_task(name: &str, retry_policy: RetryPolicy) -> AgentTask {
    let mut task = AgentTask::new(
        Metadata::new(name).unwrap(),
        TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap())
            .with_priority(TaskPriority::Normal)
            .with_retry_policy(retry_policy),
    );
    task.enqueue().unwrap();
    task
}

fn scripted_runtime() -> (Arc<SingleTurnRuntime>, Arc<ScriptedProvider>) {
    let provider = Arc::new(ScriptedProvider::new(
        ProviderName::new("test-provider").unwrap(),
        ProviderCapabilities::new([(
            ModelName::new("test-model").unwrap(),
            ModelCapabilities::new(
                NonZeroU32::new(128_000).unwrap(),
                NonZeroU32::new(4_096).unwrap(),
                false,
                true,
                1_000_000,
                2_000_000,
            ),
        )])
        .unwrap(),
    ));
    let registry = Arc::new(ProviderRegistry::new());
    let adapter: Arc<dyn ModelProvider> = provider.clone();
    registry
        .register(
            adapter,
            [(
                ModelName::new("test-model").unwrap(),
                ModelRoutingProfile::new(9_000, duration("1s"), DataResidency::Remote).unwrap(),
            )],
        )
        .unwrap();
    let router: Arc<dyn ModelRouting> = Arc::new(ModelRouter::new(registry));
    (Arc::new(SingleTurnRuntime::new(router)), provider)
}

struct Harness {
    node_id: NodeId,
    agent_id: agentkube_core::AgentId,
    task_id: agentkube_core::TaskId,
    queue: Arc<InMemoryTaskQueue>,
    state: Arc<InMemoryWorkerStateStore>,
    worker: Worker,
}

fn harness(task: AgentTask, runtime: Arc<dyn AgentRuntime>) -> Harness {
    let node_id = NodeId::new();
    let definition = definition("worker-agent");
    let agent = ready_agent(definition.metadata().uid(), node_id);
    let agent_id = agent.id();
    let task_id = task.status().task_id();
    let queue = Arc::new(InMemoryTaskQueue::new());
    ready(queue.enqueue(EnqueueRequest::from_task(&task).unwrap())).unwrap();
    let state = Arc::new(InMemoryWorkerStateStore::new());
    state.insert_definition(definition).unwrap();
    state.insert_agent(agent).unwrap();
    state.insert_task(task).unwrap();
    let queue_port: Arc<dyn TaskQueue> = queue.clone();
    let state_port = state.clone();
    let worker = Worker::new(
        node_id,
        WorkerSettings::new(NonZeroU16::new(2).unwrap(), duration("30s")),
        queue_port,
        state_port,
        runtime,
    );
    Harness {
        node_id,
        agent_id,
        task_id,
        queue,
        state,
        worker,
    }
}

#[derive(Clone)]
struct FailingRuntime {
    error: RuntimeError,
}

impl AgentRuntime for FailingRuntime {
    fn health<'a>(&'a self) -> RuntimeFuture<'a, RuntimeHealth> {
        Box::pin(async { Ok(RuntimeHealth::Healthy) })
    }

    fn execute<'a>(
        &'a self,
        _claim: agentkube_workers::ExecutionClaim,
    ) -> RuntimeFuture<'a, RuntimeOutput> {
        let error = self.error.clone();
        Box::pin(async move { Err(error) })
    }
}

struct BlockingRuntime {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl AgentRuntime for BlockingRuntime {
    fn health<'a>(&'a self) -> RuntimeFuture<'a, RuntimeHealth> {
        Box::pin(async { Ok(RuntimeHealth::Healthy) })
    }

    fn execute<'a>(
        &'a self,
        _claim: agentkube_workers::ExecutionClaim,
    ) -> RuntimeFuture<'a, RuntimeOutput> {
        Box::pin(async move {
            self.entered.wait();
            self.release.wait();
            Err(RuntimeError::Unavailable)
        })
    }
}

#[test]
fn worker_completes_a_task_through_router_and_provider() {
    let (runtime, provider) = scripted_runtime();
    provider
        .push_response(
            GenerationResponse::text(
                ModelName::new("test-model").unwrap(),
                MessageText::new("completed output").unwrap(),
                FinishReason::Stop,
                GenerationUsage::new(20, 5, 0),
            )
            .unwrap(),
        )
        .unwrap();
    let task = queued_task("successful", RetryPolicy::default());
    let harness = harness(task, runtime);

    let report = ready(harness.worker.run_once(harness.agent_id))
        .unwrap()
        .unwrap();

    let WorkerRunOutcome::Completed(output) = report.outcome() else {
        panic!("expected completed execution");
    };
    assert_eq!(output.result().output(), "completed output");
    assert_eq!(output.result().usage().cost_micro_usd(), 30);
    assert_eq!(output.route().provider().as_str(), "test-provider");
    let stored_task = harness.state.task(harness.task_id).unwrap().unwrap();
    assert_eq!(stored_task.status().state(), TaskState::Completed);
    assert_eq!(
        stored_task.status().result().unwrap().output(),
        "completed output"
    );
    assert_eq!(
        harness
            .state
            .agent(harness.agent_id)
            .unwrap()
            .unwrap()
            .state(),
        AgentInstanceState::Ready
    );
    assert_eq!(ready(harness.queue.stats()).unwrap().total(), 0);
}

#[test]
fn retryable_runtime_failure_is_persisted_and_requeued_with_backoff() {
    let retry_policy = RetryPolicy::new(NonZeroU16::new(2).unwrap())
        .with_retry_on(TaskFailureKind::RateLimit)
        .with_backoff(BackoffPolicy::Fixed {
            delay: duration("5s"),
        });
    let provider_error = ProviderError::new(
        ProviderName::new("test-provider").unwrap(),
        ProviderErrorKind::RateLimited { retry_after: None },
        "rate limited",
    );
    let runtime: Arc<dyn AgentRuntime> = Arc::new(FailingRuntime {
        error: RuntimeError::Routing(RouterError::AllProvidersFailed(vec![provider_error])),
    });
    let harness = harness(queued_task("retryable", retry_policy), runtime);

    let report = ready(harness.worker.run_once(harness.agent_id))
        .unwrap()
        .unwrap();

    assert!(matches!(
        report.outcome(),
        WorkerRunOutcome::Requeued {
            delay: Some(value),
            ..
        } if *value == duration("5s")
    ));
    assert_eq!(
        harness
            .state
            .task(harness.task_id)
            .unwrap()
            .unwrap()
            .status()
            .state(),
        TaskState::Queued
    );
    assert_eq!(
        harness
            .state
            .agent(harness.agent_id)
            .unwrap()
            .unwrap()
            .state(),
        AgentInstanceState::Failed
    );
    assert_eq!(ready(harness.queue.stats()).unwrap().delayed(), 1);
}

#[test]
fn validation_failure_is_terminal_and_acknowledged() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(FailingRuntime {
        error: RuntimeError::UnsupportedTools,
    });
    let harness = harness(queued_task("terminal", RetryPolicy::default()), runtime);

    let report = ready(harness.worker.run_once(harness.agent_id))
        .unwrap()
        .unwrap();

    assert!(matches!(report.outcome(), WorkerRunOutcome::Failed(_)));
    let task = harness.state.task(harness.task_id).unwrap().unwrap();
    assert_eq!(task.status().state(), TaskState::Failed);
    assert_eq!(
        task.status().failure().unwrap().kind(),
        TaskFailureKind::Validation
    );
    assert_eq!(ready(harness.queue.stats()).unwrap().total(), 0);
}

#[test]
fn stale_queue_delivery_is_discarded_without_runtime_execution() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(FailingRuntime {
        error: RuntimeError::Unavailable,
    });
    let task = queued_task("missing", RetryPolicy::default());
    let missing_id = task.status().task_id();
    let node_id = NodeId::new();
    let definition = definition("discard-agent");
    let agent = ready_agent(definition.metadata().uid(), node_id);
    let agent_id = agent.id();
    let queue = Arc::new(InMemoryTaskQueue::new());
    ready(queue.enqueue(EnqueueRequest::from_task(&task).unwrap())).unwrap();
    let state = Arc::new(InMemoryWorkerStateStore::new());
    state.insert_definition(definition).unwrap();
    state.insert_agent(agent).unwrap();
    let queue_port: Arc<dyn TaskQueue> = queue.clone();
    let worker = Worker::new(
        node_id,
        WorkerSettings::new(NonZeroU16::new(1).unwrap(), duration("30s")),
        queue_port,
        state,
        runtime,
    );

    let report = ready(worker.run_once(agent_id)).unwrap().unwrap();

    assert_eq!(report.task_id(), missing_id);
    assert_eq!(
        report.outcome(),
        &WorkerRunOutcome::Discarded(DiscardReason::TaskMissing)
    );
    assert_eq!(ready(queue.stats()).unwrap().total(), 0);
}

#[test]
fn wrong_node_releases_delivery_and_preserves_domain_state() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(FailingRuntime {
        error: RuntimeError::Unavailable,
    });
    let mut harness = harness(
        queued_task("wrong-node", RetryPolicy::default()),
        runtime.clone(),
    );
    let other_node = NodeId::new();
    let queue_port: Arc<dyn TaskQueue> = harness.queue.clone();
    let state_port = harness.state.clone();
    harness.worker = Worker::new(
        other_node,
        WorkerSettings::new(NonZeroU16::new(1).unwrap(), duration("30s")),
        queue_port,
        state_port,
        runtime,
    );

    assert!(matches!(
        ready(harness.worker.run_once(harness.agent_id)),
        Err(WorkerError::State(
            agentkube_workers::WorkerStateError::WrongNode { .. }
        ))
    ));
    assert_eq!(
        harness
            .state
            .task(harness.task_id)
            .unwrap()
            .unwrap()
            .status()
            .state(),
        TaskState::Queued
    );
    assert_eq!(ready(harness.queue.stats()).unwrap().ready(), 1);
}

#[test]
fn heartbeats_are_monotonic_and_report_capacity() {
    let runtime: Arc<dyn AgentRuntime> = Arc::new(FailingRuntime {
        error: RuntimeError::Unavailable,
    });
    let harness = harness(queued_task("heartbeat", RetryPolicy::default()), runtime);

    let first = harness.worker.heartbeat().unwrap();
    let second = harness.worker.heartbeat().unwrap();

    assert_eq!(first.node_id(), harness.node_id);
    assert_eq!(first.sequence(), 1);
    assert_eq!(second.sequence(), 2);
    assert!(second.observed_at() >= first.observed_at());
    assert_eq!(second.active_executions(), 0);
    assert_eq!(second.capacity().get(), 2);
}

#[test]
fn worker_enforces_capacity_and_heartbeats_report_active_execution() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let runtime: Arc<dyn AgentRuntime> = Arc::new(BlockingRuntime {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    });
    let mut harness = harness(
        queued_task("capacity", RetryPolicy::default()),
        Arc::clone(&runtime),
    );
    let queue_port: Arc<dyn TaskQueue> = harness.queue.clone();
    let state_port = harness.state.clone();
    harness.worker = Worker::new(
        harness.node_id,
        WorkerSettings::new(NonZeroU16::new(1).unwrap(), duration("30s")),
        queue_port,
        state_port,
        runtime,
    );
    let worker = Arc::new(harness.worker);
    let background_worker = Arc::clone(&worker);
    let agent_id = harness.agent_id;
    let execution = std::thread::spawn(move || ready(background_worker.run_once(agent_id)));

    entered.wait();
    assert_eq!(worker.heartbeat().unwrap().active_executions(), 1);
    assert_eq!(
        ready(worker.run_once(agent_id)),
        Err(WorkerError::AtCapacity)
    );
    release.wait();
    let report = execution.join().unwrap().unwrap().unwrap();
    assert!(matches!(report.outcome(), WorkerRunOutcome::Failed(_)));
    assert_eq!(worker.heartbeat().unwrap().active_executions(), 0);
}
