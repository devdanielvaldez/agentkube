//! Scheduled task dispatch with real model execution.
//!
//! Each cycle lists queued tasks by priority, builds scheduling candidates
//! from live instances, places every task with the real
//! [`Scheduler`](agentkube_scheduler::Scheduler), and executes winners
//! through the provider runtime. Scheduling strictly precedes claiming: the
//! worker state store pairs the leased task with the decided agent, and a
//! queue race (the lease holding another task) releases the delivery instead
//! of executing a mismatched placement.

use crate::OperatorError;
use agentkube_agents::ProviderName;
use agentkube_api::{NodeInfo, NodeRegistry};
use agentkube_core::{HumanDuration, NodeId, Resource};
use agentkube_queue::TaskQueue;
use agentkube_scheduler::{
    CandidateMetrics, NodeLocality, NodeSnapshot, ProviderAvailability, ScheduleOutcome, Scheduler,
    SchedulingCandidate,
};
use agentkube_storage::ResourceRepository;
use agentkube_tasks::{AgentTask, TaskFailure, TaskState};
use agentkube_workers::{AgentRuntime, RepositoryWorkerStateStore, WorkerStateStore};
use std::sync::{
    Arc,
    atomic::{AtomicU16, Ordering},
};

/// Provider endpoint advertised by the local node.
#[derive(Debug, Clone)]
pub struct ProviderDescriptor {
    /// Registered provider name.
    pub name: ProviderName,
    /// Whether execution stays local.
    pub locality: NodeLocality,
    /// Whether confidential inputs may flow here.
    pub accepts_confidential: bool,
}

impl ProviderDescriptor {
    /// Describes one reachable provider endpoint.
    #[must_use]
    pub const fn new(
        name: ProviderName,
        locality: NodeLocality,
        accepts_confidential: bool,
    ) -> Self {
        Self {
            name,
            locality,
            accepts_confidential,
        }
    }
}

/// Counts from one dispatch cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DispatchSummary {
    /// Tasks that reached execution (completed, failed, or requeued).
    pub executed: usize,
    /// Tasks no candidate could satisfy (left queued with a warning).
    pub unscheduled: usize,
    /// Leases released after losing a queue race.
    pub races: usize,
}

/// Scheduled dispatch over repository, queue, and runtime ports.
pub struct Dispatcher {
    tasks: Arc<dyn ResourceRepository<AgentTask>>,
    agents: Arc<dyn ResourceRepository<agentkube_agents::AgentDefinition>>,
    queue: Arc<dyn TaskQueue>,
    state: RepositoryWorkerStateStore,
    runtime: Arc<dyn AgentRuntime>,
    scheduler: Scheduler,
    node_id: NodeId,
    providers: Vec<ProviderDescriptor>,
    concurrency: u16,
    lease_duration: HumanDuration,
    nodes: Arc<NodeRegistry>,
    active: Arc<AtomicU16>,
}

impl Dispatcher {
    /// Creates a dispatcher. The node advertises `providers` with
    /// `concurrency` execution slots; `active` tracks in-flight executions
    /// for heartbeats and must start at zero.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        tasks: Arc<dyn ResourceRepository<AgentTask>>,
        agents: Arc<dyn ResourceRepository<agentkube_agents::AgentDefinition>>,
        queue: Arc<dyn TaskQueue>,
        state: RepositoryWorkerStateStore,
        runtime: Arc<dyn AgentRuntime>,
        scheduler: Scheduler,
        node_id: NodeId,
        providers: Vec<ProviderDescriptor>,
        concurrency: u16,
        lease_duration: HumanDuration,
        nodes: Arc<NodeRegistry>,
        active: Arc<AtomicU16>,
    ) -> Self {
        Self {
            tasks,
            agents,
            queue,
            state,
            runtime,
            scheduler,
            node_id,
            providers,
            concurrency,
            lease_duration,
            nodes,
            active,
        }
    }

    /// Runs one dispatch cycle: schedule every queued task, execute winners.
    pub async fn dispatch_once(&self) -> Result<DispatchSummary, OperatorError> {
        use agentkube_workers::RuntimeHealth;
        if self
            .runtime
            .health()
            .await
            .map_err(|error| OperatorError::Worker(format!("runtime unhealthy: {error}")))?
            != RuntimeHealth::Healthy
        {
            eprintln!("dispatch: runtime not healthy, skipping cycle");
            return Ok(DispatchSummary::default());
        }
        let definitions = self.agents.list(None).await?;
        self.state
            .sync_definitions(definitions.clone())
            .map_err(|error| OperatorError::Worker(error.to_string()))?;
        for definition in &definitions {
            // Capacity is explicit: one ready instance per definition on this
            // node. Failures here skip the definition loudly; they never abort
            // dispatch for every other definition.
            if let Err(error) = self.state.ensure_instance(definition, self.node_id) {
                eprintln!(
                    "dispatch: cannot ensure instance for {:?}: {error}",
                    definition.metadata().name().as_str()
                );
            }
        }
        let mut tasks: Vec<AgentTask> = self
            .tasks
            .list(None)
            .await?
            .into_iter()
            .filter(|task| task.status().state() == TaskState::Queued)
            .collect();
        tasks.sort_by(|left, right| {
            right
                .spec()
                .priority()
                .weight()
                .cmp(&left.spec().priority().weight())
                .then_with(|| {
                    left.metadata()
                        .name()
                        .as_str()
                        .cmp(right.metadata().name().as_str())
                })
        });

        let mut summary = DispatchSummary::default();
        for task in &tasks {
            let candidates = self.candidates(&definitions).await?;
            match self.scheduler.schedule(task, &candidates) {
                Err(error) => {
                    return Err(OperatorError::Scheduling(format!(
                        "cannot schedule task {:?}: {error}",
                        task.metadata().name().as_str()
                    )));
                }
                Ok(ScheduleOutcome::Unschedulable(report)) => {
                    summary.unscheduled += 1;
                    eprintln!(
                        "dispatch: task {:?} unschedulable ({} candidates rejected)",
                        task.metadata().name().as_str(),
                        report.evaluations().len()
                    );
                }
                Ok(ScheduleOutcome::Selected { decision, .. }) => {
                    self.execute_decided(&decision, &mut summary).await?;
                }
            }
        }
        self.record_heartbeat().await;
        if summary.executed > 0 || summary.unscheduled > 0 || summary.races > 0 {
            eprintln!(
                "dispatch: executed {}, unscheduled {}, races {}",
                summary.executed, summary.unscheduled, summary.races
            );
        }
        Ok(summary)
    }

    async fn candidates(
        &self,
        definitions: &[agentkube_agents::AgentDefinition],
    ) -> Result<Vec<SchedulingCandidate>, OperatorError> {
        let availability: Vec<ProviderAvailability> = self
            .providers
            .iter()
            .map(|provider| {
                ProviderAvailability::new(
                    provider.name.clone(),
                    provider.locality,
                    provider.accepts_confidential,
                )
            })
            .collect();
        let node = NodeSnapshot::new(
            self.node_id,
            true,
            true,
            self.active.load(Ordering::SeqCst),
            self.concurrency,
            availability,
        )
        .map_err(|error| OperatorError::Scheduling(format!("invalid node snapshot: {error}")))?;
        let instances = self
            .state
            .instances()
            .map_err(|error| OperatorError::Worker(error.to_string()))?;
        let mut candidates = Vec::new();
        for definition in definitions {
            let owner = definition.metadata().uid();
            let Some(instance) = instances
                .iter()
                .find(|instance| instance.definition_uid() == owner)
            else {
                continue;
            };
            let metrics =
                CandidateMetrics::new(5_000, Self::neutral_latency(), 0).map_err(|error| {
                    OperatorError::Scheduling(format!("invalid candidate metrics: {error}"))
                })?;
            match SchedulingCandidate::new(
                instance.clone(),
                definition.clone(),
                node.clone(),
                metrics,
            ) {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => {
                    eprintln!(
                        "dispatch: skipping candidate for definition {:?}: {error}",
                        definition.metadata().name().as_str()
                    );
                }
            }
        }
        Ok(candidates)
    }

    /// Neutral priors until telemetry exists: uniform quality with documented
    /// latency and zero cost. Hard constraints (not scores) do the filtering.
    fn neutral_latency() -> HumanDuration {
        "1s".parse().expect("static latency is valid")
    }

    async fn execute_decided(
        &self,
        decision: &agentkube_scheduler::PlacementDecision,
        summary: &mut DispatchSummary,
    ) -> Result<(), OperatorError> {
        let Some(lease) = self.queue.lease(self.node_id, self.lease_duration).await? else {
            return Ok(());
        };
        if lease.task_id() != decision.task_id() {
            // Another task won the queue race after scheduling: release this
            // delivery untouched instead of executing a mismatched placement.
            self.queue
                .release(lease.lease_id(), self.node_id, None)
                .await?;
            summary.races += 1;
            return Ok(());
        }
        let task_id = decision.task_id();
        let agent_id = decision.agent_id();
        let claim = match self.state.claim(task_id, agent_id, self.node_id).await {
            Ok(claim) => claim,
            Err(error) if is_stale_delivery(&error) => {
                self.queue
                    .acknowledge(lease.lease_id(), self.node_id)
                    .await?;
                eprintln!("dispatch: discarded stale delivery for task {task_id}: {error}");
                return Ok(());
            }
            Err(error) => {
                self.queue
                    .release(lease.lease_id(), self.node_id, None)
                    .await?;
                return Err(OperatorError::Worker(error.to_string()));
            }
        };
        self.active.fetch_add(1, Ordering::SeqCst);
        self.record_heartbeat().await;
        let outcome = self.runtime.execute(claim).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        match outcome {
            Ok(output) => {
                self.state
                    .complete(task_id, agent_id, output.result().clone())
                    .await
                    .map_err(|error| OperatorError::Worker(error.to_string()))?;
                self.queue
                    .acknowledge(lease.lease_id(), self.node_id)
                    .await?;
                summary.executed += 1;
            }
            Err(error) => {
                let failure = TaskFailure::new(error.task_failure_kind(), error.to_string());
                match self.state.fail(task_id, agent_id, failure.clone()).await {
                    Ok(agentkube_workers::FailureDisposition::Requeued { delay }) => {
                        self.queue
                            .release(lease.lease_id(), self.node_id, delay)
                            .await?;
                        summary.executed += 1;
                    }
                    Ok(agentkube_workers::FailureDisposition::Terminal) => {
                        self.queue
                            .acknowledge(lease.lease_id(), self.node_id)
                            .await?;
                        summary.executed += 1;
                    }
                    Err(state_error) => {
                        self.queue
                            .release(lease.lease_id(), self.node_id, None)
                            .await?;
                        return Err(OperatorError::Worker(state_error.to_string()));
                    }
                }
            }
        }
        self.record_heartbeat().await;
        Ok(())
    }

    async fn record_heartbeat(&self) {
        self.nodes
            .record_heartbeat(NodeInfo::new(
                self.node_id,
                self.active.load(Ordering::SeqCst),
                self.concurrency_nonzero(),
            ))
            .await;
    }

    fn concurrency_nonzero(&self) -> std::num::NonZeroU16 {
        std::num::NonZeroU16::new(self.concurrency.max(1)).expect("concurrency is non-zero")
    }
}

fn is_stale_delivery(error: &agentkube_workers::WorkerStateError) -> bool {
    use agentkube_workers::WorkerStateError;
    matches!(
        error,
        WorkerStateError::TaskNotFound(_) | WorkerStateError::TaskNotQueued { .. }
    )
}
