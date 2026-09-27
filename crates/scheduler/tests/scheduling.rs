use agentkube_agents::{
    AgentDefinition, AgentInstance, AgentInstanceState, AgentResourceLimits, AgentRole, AgentSpec,
    Capability, Instructions, ModelName, ModelPolicy, OptimizationObjective, ProviderName,
    ToolName,
};
use agentkube_core::{HumanDuration, Metadata, NodeId, Resource};
use agentkube_scheduler::{
    BasisPoints, CandidateError, CandidateMetrics, NodeLocality, NodeSnapshot,
    ProviderAvailability, RejectionReason, ScheduleOutcome, Scheduler, SchedulerConfigError,
    SchedulingCandidate, SchedulingError, ScoreWeights,
};
use agentkube_tasks::{
    AgentTask, Objective, PrivacyRequirement, TaskBudget, TaskRequirements, TaskState,
};
use std::num::NonZeroU64;

fn duration(value: &str) -> HumanDuration {
    value.parse().expect("test duration is valid")
}

fn provider(name: &str, locality: NodeLocality, confidential: bool) -> ProviderAvailability {
    ProviderAvailability::new(
        ProviderName::new(name).expect("test provider is valid"),
        locality,
        confidential,
    )
}

fn queued_task(requirements: TaskRequirements, budget: TaskBudget) -> AgentTask {
    let mut task = AgentTask::new(
        Metadata::new("test-task").unwrap(),
        agentkube_tasks::TaskSpec::new(Objective::new("Ship a robust feature").unwrap())
            .with_requirements(requirements)
            .with_budget(budget),
    );
    task.enqueue().unwrap();
    task
}

struct CandidateInput<'a> {
    name: &'a str,
    node_id: NodeId,
    providers: Vec<ProviderAvailability>,
    capabilities: &'a [&'a str],
    tools: &'a [&'a str],
    quality: u16,
    latency: &'a str,
    cost: u64,
    used_slots: u16,
    total_slots: u16,
}

fn candidate(input: CandidateInput<'_>) -> SchedulingCandidate {
    let mut spec = AgentSpec::new(
        AgentRole::new("developer").unwrap(),
        ModelPolicy::automatic(
            [],
            [OptimizationObjective::Quality, OptimizationObjective::Cost],
        )
        .unwrap(),
        Instructions::new("Build production software.").unwrap(),
    );
    for capability in input.capabilities {
        spec = spec.with_capability(Capability::new(*capability).unwrap());
    }
    for tool in input.tools {
        spec = spec.with_tool(ToolName::new(*tool).unwrap());
    }
    let definition = AgentDefinition::new(Metadata::new(input.name).unwrap(), spec);
    let mut instance = AgentInstance::new(definition.metadata().uid());
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(input.node_id).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
    let node = NodeSnapshot::new(
        input.node_id,
        true,
        true,
        input.used_slots,
        input.total_slots,
        input.providers,
    )
    .unwrap();
    SchedulingCandidate::new(
        instance,
        definition,
        node,
        CandidateMetrics::new(input.quality, duration(input.latency), input.cost).unwrap(),
    )
    .unwrap()
}

#[test]
fn scheduler_filters_constraints_then_selects_highest_weighted_score() {
    let requirements = TaskRequirements::new()
        .with_capability(Capability::new("coding").unwrap())
        .with_tool(ToolName::new("git").unwrap())
        .with_preferred_role(AgentRole::new("developer").unwrap());
    let task = queued_task(requirements, TaskBudget::new());
    let quality = candidate(CandidateInput {
        name: "quality-agent",
        node_id: NodeId::new(),
        providers: vec![provider("openai", NodeLocality::Cloud, true)],
        capabilities: &["coding"],
        tools: &["git"],
        quality: 9_800,
        latency: "2s",
        cost: 20_000,
        used_slots: 1,
        total_slots: 4,
    });
    let cheap = candidate(CandidateInput {
        name: "cheap-agent",
        node_id: NodeId::new(),
        providers: vec![provider("local", NodeLocality::Local, true)],
        capabilities: &["coding"],
        tools: &["git"],
        quality: 7_000,
        latency: "2s",
        cost: 10_000,
        used_slots: 1,
        total_slots: 4,
    });

    let outcome = Scheduler::default()
        .schedule(&task, &[quality.clone(), cheap])
        .unwrap();

    let decision = outcome.decision().expect("a placement should win");
    assert_eq!(decision.agent_id(), quality.instance().id());
    assert_eq!(decision.provider().as_str(), "openai");
    assert_eq!(outcome.evaluations().len(), 2);
    assert!(outcome.evaluations().iter().all(|item| item.is_feasible()));
}

#[test]
fn scheduler_reports_every_hard_constraint_failure() {
    let requirements = TaskRequirements::new()
        .with_capability(Capability::new("coding").unwrap())
        .with_tool(ToolName::new("git").unwrap())
        .with_privacy(PrivacyRequirement::LocalOnly)
        .with_max_latency(duration("2s"));
    let budget = TaskBudget::new().with_max_cost_micro_usd(NonZeroU64::new(100).unwrap());
    let task = queued_task(requirements, budget);
    let incompatible = candidate(CandidateInput {
        name: "incompatible-agent",
        node_id: NodeId::new(),
        providers: vec![provider("openai", NodeLocality::Cloud, false)],
        capabilities: &[],
        tools: &[],
        quality: 9_000,
        latency: "5s",
        cost: 200,
        used_slots: 1,
        total_slots: 1,
    });

    let outcome = Scheduler::default()
        .schedule(&task, &[incompatible])
        .unwrap();
    let ScheduleOutcome::Unschedulable(report) = outcome else {
        panic!("candidate must be rejected");
    };
    let reasons = report.evaluations()[0].rejections();
    assert_eq!(reasons.len(), 6);
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, RejectionReason::NodeAtCapacity))
    );
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, RejectionReason::MissingCapabilities(_)))
    );
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, RejectionReason::MissingTools(_)))
    );
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, RejectionReason::NoEligibleProvider))
    );
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, RejectionReason::LatencyExceeded { .. }))
    );
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, RejectionReason::CostExceeded { .. }))
    );
}

#[test]
fn provider_policy_combines_model_allowlist_task_denylist_and_privacy() {
    let anthropic = ProviderName::new("anthropic").unwrap();
    let local = ProviderName::new("local").unwrap();
    let requirements = TaskRequirements::new()
        .deny_provider(anthropic.clone())
        .unwrap()
        .with_privacy(PrivacyRequirement::LocalOnly);
    let task = queued_task(requirements, TaskBudget::new());
    let node_id = NodeId::new();
    let mut spec = AgentSpec::new(
        AgentRole::new("developer").unwrap(),
        ModelPolicy::automatic([anthropic], [OptimizationObjective::Privacy]).unwrap(),
        Instructions::new("Keep data private.").unwrap(),
    );
    spec = spec.with_capability(Capability::new("coding").unwrap());
    let definition = AgentDefinition::new(Metadata::new("private-agent").unwrap(), spec);
    let mut instance = AgentInstance::new(definition.metadata().uid());
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(node_id).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
    let node = NodeSnapshot::new(
        node_id,
        true,
        true,
        0,
        1,
        [
            provider("anthropic", NodeLocality::Cloud, true),
            ProviderAvailability::new(local, NodeLocality::Local, true),
        ],
    )
    .unwrap();
    let candidate = SchedulingCandidate::new(
        instance,
        definition,
        node,
        CandidateMetrics::new(9_000, duration("1s"), 10).unwrap(),
    )
    .unwrap();

    let outcome = Scheduler::default().schedule(&task, &[candidate]).unwrap();
    assert!(matches!(outcome, ScheduleOutcome::Unschedulable(_)));
}

#[test]
fn fixed_model_must_be_reachable_from_the_node() {
    let node_id = NodeId::new();
    let definition = AgentDefinition::new(
        Metadata::new("fixed-agent").unwrap(),
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("openai").unwrap(),
                ModelName::new("gpt-5").unwrap(),
            ),
            Instructions::new("Use the fixed model.").unwrap(),
        ),
    );
    let mut instance = AgentInstance::new(definition.metadata().uid());
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(node_id).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
    let node = NodeSnapshot::new(
        node_id,
        true,
        true,
        0,
        1,
        [provider("anthropic", NodeLocality::Cloud, true)],
    )
    .unwrap();
    let candidate = SchedulingCandidate::new(
        instance,
        definition,
        node,
        CandidateMetrics::new(9_000, duration("1s"), 10).unwrap(),
    )
    .unwrap();

    let task = queued_task(TaskRequirements::new(), TaskBudget::new());
    let outcome = Scheduler::default().schedule(&task, &[candidate]).unwrap();
    assert!(matches!(outcome, ScheduleOutcome::Unschedulable(_)));
}

#[test]
fn failed_agents_and_unhealthy_cordoned_nodes_are_rejected() {
    let node_id = NodeId::new();
    let definition = AgentDefinition::new(
        Metadata::new("unavailable-agent").unwrap(),
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("openai").unwrap(),
                ModelName::new("gpt-5").unwrap(),
            ),
            Instructions::new("Do not receive new work.").unwrap(),
        ),
    );
    let mut instance = AgentInstance::new(definition.metadata().uid());
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(node_id).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
    instance.fail().unwrap();
    let node = NodeSnapshot::new(
        node_id,
        false,
        false,
        0,
        1,
        [provider("openai", NodeLocality::Cloud, true)],
    )
    .unwrap();
    let candidate = SchedulingCandidate::new(
        instance,
        definition,
        node,
        CandidateMetrics::new(9_000, duration("1s"), 10).unwrap(),
    )
    .unwrap();
    let task = queued_task(TaskRequirements::new(), TaskBudget::new());

    let outcome = Scheduler::default().schedule(&task, &[candidate]).unwrap();
    let reasons = outcome.evaluations()[0].rejections();
    assert!(matches!(
        reasons[0],
        RejectionReason::AgentNotReady(AgentInstanceState::Failed)
    ));
    assert_eq!(reasons[1], RejectionReason::NodeNotReady);
    assert_eq!(reasons[2], RejectionReason::NodeUnschedulable);
}

#[test]
fn tighter_agent_limits_are_enforced_alongside_task_budgets() {
    let node_id = NodeId::new();
    let resources = AgentResourceLimits::new()
        .with_max_cost_micro_usd(NonZeroU64::new(50).unwrap())
        .with_timeout(duration("1s"));
    let definition = AgentDefinition::new(
        Metadata::new("bounded-agent").unwrap(),
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("openai").unwrap(),
                ModelName::new("gpt-5").unwrap(),
            ),
            Instructions::new("Stay within limits.").unwrap(),
        )
        .with_resources(resources),
    );
    let mut instance = AgentInstance::new(definition.metadata().uid());
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(node_id).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
    let node = NodeSnapshot::new(
        node_id,
        true,
        true,
        0,
        1,
        [provider("openai", NodeLocality::Cloud, true)],
    )
    .unwrap();
    let candidate = SchedulingCandidate::new(
        instance,
        definition,
        node,
        CandidateMetrics::new(9_000, duration("2s"), 75).unwrap(),
    )
    .unwrap();
    let task = queued_task(
        TaskRequirements::new(),
        TaskBudget::new()
            .with_max_cost_micro_usd(NonZeroU64::new(100).unwrap())
            .with_timeout(duration("3s")),
    );

    let outcome = Scheduler::default().schedule(&task, &[candidate]).unwrap();
    let reasons = outcome.evaluations()[0].rejections();
    assert!(
        matches!(reasons[0], RejectionReason::LatencyExceeded { maximum, .. } if maximum == duration("1s"))
    );
    assert!(matches!(
        reasons[1],
        RejectionReason::CostExceeded {
            maximum_micro_usd: 50,
            ..
        }
    ));
}

#[test]
fn ties_are_deterministic_regardless_of_input_order() {
    let task = queued_task(TaskRequirements::new(), TaskBudget::new());
    let first = candidate(CandidateInput {
        name: "first-agent",
        node_id: NodeId::new(),
        providers: vec![provider("openai", NodeLocality::Cloud, true)],
        capabilities: &[],
        tools: &[],
        quality: 9_000,
        latency: "1s",
        cost: 100,
        used_slots: 0,
        total_slots: 1,
    });
    let second = candidate(CandidateInput {
        name: "second-agent",
        node_id: NodeId::new(),
        providers: vec![provider("openai", NodeLocality::Cloud, true)],
        capabilities: &[],
        tools: &[],
        quality: 9_000,
        latency: "1s",
        cost: 100,
        used_slots: 0,
        total_slots: 1,
    });

    let forward = Scheduler::default()
        .schedule(&task, &[first.clone(), second.clone()])
        .unwrap();
    let reverse = Scheduler::default()
        .schedule(&task, &[second, first])
        .unwrap();
    assert_eq!(
        forward.decision().unwrap().agent_id(),
        reverse.decision().unwrap().agent_id()
    );
}

#[test]
fn task_must_enter_the_queue_before_scheduling() {
    let task = AgentTask::new(
        Metadata::new("pending-task").unwrap(),
        agentkube_tasks::TaskSpec::new(Objective::new("Not queued").unwrap()),
    );
    assert_eq!(task.status().state(), TaskState::Pending);
    assert_eq!(
        Scheduler::default().schedule(&task, &[]),
        Err(SchedulingError::TaskNotQueued(TaskState::Pending))
    );
}

#[test]
fn configuration_and_candidate_snapshots_reject_invalid_input() {
    let invalid_weights = ScoreWeights::new(
        BasisPoints::new(1_000).unwrap(),
        BasisPoints::new(1_000).unwrap(),
        BasisPoints::new(1_000).unwrap(),
        BasisPoints::new(1_000).unwrap(),
        BasisPoints::new(1_000).unwrap(),
    );
    assert_eq!(
        invalid_weights,
        Err(SchedulerConfigError::InvalidWeightTotal(5_000))
    );
    assert!(matches!(
        NodeSnapshot::new(NodeId::new(), true, true, 2, 1, []),
        Err(CandidateError::InvalidCapacity { .. })
    ));
    assert!(CandidateMetrics::new(10_001, duration("1s"), 1).is_err());
}
