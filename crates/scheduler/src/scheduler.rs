use crate::{
    BasisPoints, NodeLocality, ProviderAvailability, ScheduleOutcome::Unschedulable,
    SchedulingCandidate, SchedulingError, ScoreBreakdown, ScoreWeights,
};
use agentkube_agents::{AgentInstanceState, Capability, ModelPolicy, ProviderName, ToolName};
use agentkube_core::{AgentId, HumanDuration, NodeId, TaskId};
use agentkube_tasks::{AgentTask, PrivacyRequirement, TaskState};
use std::cmp::Ordering;

/// A hard constraint that excluded an agent/node candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// Agent instance is not idle and ready.
    AgentNotReady(AgentInstanceState),
    /// Node health is not ready.
    NodeNotReady,
    /// Node is cordoned or draining.
    NodeUnschedulable,
    /// Node has no free execution slots.
    NodeAtCapacity,
    /// Agent definition lacks required capabilities.
    MissingCapabilities(Vec<Capability>),
    /// Agent definition lacks required tools.
    MissingTools(Vec<ToolName>),
    /// No provider satisfies model, allow/deny, and privacy policy.
    NoEligibleProvider,
    /// Historical latency exceeds the tightest workload or agent limit.
    LatencyExceeded {
        /// Estimated latency.
        expected: HumanDuration,
        /// Effective upper bound.
        maximum: HumanDuration,
    },
    /// Historical cost exceeds the tightest workload or agent limit.
    CostExceeded {
        /// Estimated cost in millionths of one US dollar.
        expected_micro_usd: u64,
        /// Effective upper bound in millionths of one US dollar.
        maximum_micro_usd: u64,
    },
}

/// Explainable result of evaluating one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEvaluation {
    agent_id: AgentId,
    node_id: NodeId,
    provider: Option<ProviderName>,
    rejections: Vec<RejectionReason>,
    score: Option<ScoreBreakdown>,
}

impl CandidateEvaluation {
    /// Evaluated agent identifier.
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }
    /// Evaluated node identifier.
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }
    /// Deterministically selected eligible provider, if any.
    #[must_use]
    pub const fn provider(&self) -> Option<&ProviderName> {
        self.provider.as_ref()
    }
    /// Hard-constraint failures. Empty means the candidate was feasible.
    #[must_use]
    pub fn rejections(&self) -> &[RejectionReason] {
        &self.rejections
    }
    /// Weighted score for a feasible candidate.
    #[must_use]
    pub const fn score(&self) -> Option<ScoreBreakdown> {
        self.score
    }
    /// Whether the candidate passed every hard constraint.
    #[must_use]
    pub fn is_feasible(&self) -> bool {
        self.rejections.is_empty()
    }
}

/// Winning placement selected for a queued task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementDecision {
    task_id: TaskId,
    agent_id: AgentId,
    node_id: NodeId,
    provider: ProviderName,
    score: ScoreBreakdown,
}

impl PlacementDecision {
    /// Task being placed.
    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }
    /// Selected agent instance.
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }
    /// Selected worker node.
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }
    /// Provider satisfying placement policy.
    #[must_use]
    pub const fn provider(&self) -> &ProviderName {
        &self.provider
    }
    /// Winning score breakdown.
    #[must_use]
    pub const fn score(&self) -> ScoreBreakdown {
        self.score
    }
}

/// Diagnostic produced when every candidate violates a hard constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnschedulableReport {
    task_id: TaskId,
    evaluations: Vec<CandidateEvaluation>,
}

impl UnschedulableReport {
    /// Task that could not be placed.
    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }
    /// Candidate-level rejection details.
    #[must_use]
    pub fn evaluations(&self) -> &[CandidateEvaluation] {
        &self.evaluations
    }
}

/// Expected outcome of one deterministic scheduling cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleOutcome {
    /// A feasible candidate won the weighted ranking.
    Selected {
        /// Winning placement.
        decision: PlacementDecision,
        /// Evaluation of every supplied candidate.
        evaluations: Vec<CandidateEvaluation>,
    },
    /// No candidate satisfied all hard constraints.
    Unschedulable(UnschedulableReport),
}

impl ScheduleOutcome {
    /// Returns the decision when placement succeeded.
    #[must_use]
    pub const fn decision(&self) -> Option<&PlacementDecision> {
        match self {
            Self::Selected { decision, .. } => Some(decision),
            Self::Unschedulable(_) => None,
        }
    }

    /// Returns all candidate evaluations regardless of outcome.
    #[must_use]
    pub fn evaluations(&self) -> &[CandidateEvaluation] {
        match self {
            Self::Selected { evaluations, .. } => evaluations,
            Self::Unschedulable(report) => report.evaluations(),
        }
    }
}

/// Constraint-aware, deterministic agent and node scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scheduler {
    weights: ScoreWeights,
}

impl Scheduler {
    /// Creates a scheduler with validated scoring weights.
    #[must_use]
    pub const fn new(weights: ScoreWeights) -> Self {
        Self { weights }
    }

    /// Returns active scoring weights.
    #[must_use]
    pub const fn weights(self) -> ScoreWeights {
        self.weights
    }

    /// Filters, ranks, and selects from a consistent candidate snapshot.
    pub fn schedule(
        &self,
        task: &AgentTask,
        candidates: &[SchedulingCandidate],
    ) -> Result<ScheduleOutcome, SchedulingError> {
        if task.status().state() != TaskState::Queued {
            return Err(SchedulingError::TaskNotQueued(task.status().state()));
        }

        let mut working: Vec<WorkingEvaluation> = candidates
            .iter()
            .map(|candidate| evaluate_constraints(task, candidate))
            .collect();
        let feasible_indexes: Vec<usize> = working
            .iter()
            .enumerate()
            .filter_map(|(index, evaluation)| evaluation.rejections.is_empty().then_some(index))
            .collect();

        if feasible_indexes.is_empty() {
            let mut evaluations = finalize_evaluations(working);
            sort_evaluations(&mut evaluations);
            return Ok(Unschedulable(UnschedulableReport {
                task_id: task.status().task_id(),
                evaluations,
            }));
        }

        let minimum_latency = feasible_indexes
            .iter()
            .map(|index| working[*index].latency_millis)
            .min()
            .expect("at least one feasible candidate");
        let minimum_cost = feasible_indexes
            .iter()
            .map(|index| working[*index].cost_micro_usd)
            .min()
            .expect("at least one feasible candidate");

        for index in feasible_indexes {
            working[index].score = Some(score_candidate(
                self.weights,
                &working[index],
                minimum_latency,
                minimum_cost,
            )?);
        }

        let winner = working
            .iter()
            .filter(|evaluation| evaluation.score.is_some())
            .max_by(compare_feasible)
            .expect("at least one feasible candidate");
        let decision = PlacementDecision {
            task_id: task.status().task_id(),
            agent_id: winner.agent_id,
            node_id: winner.node_id,
            provider: winner
                .provider
                .clone()
                .expect("feasible candidate has provider"),
            score: winner.score.expect("feasible candidate has score"),
        };
        let mut evaluations = finalize_evaluations(working);
        sort_evaluations(&mut evaluations);
        Ok(ScheduleOutcome::Selected {
            decision,
            evaluations,
        })
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(ScoreWeights::default())
    }
}

struct WorkingEvaluation {
    agent_id: AgentId,
    node_id: NodeId,
    provider: Option<ProviderName>,
    rejections: Vec<RejectionReason>,
    score: Option<ScoreBreakdown>,
    quality: BasisPoints,
    capability: BasisPoints,
    availability: BasisPoints,
    latency_millis: u128,
    cost_micro_usd: u64,
}

fn evaluate_constraints(task: &AgentTask, candidate: &SchedulingCandidate) -> WorkingEvaluation {
    let instance = candidate.instance();
    let definition = candidate.definition();
    let node = candidate.node();
    let requirements = task.spec().requirements();
    let metrics = candidate.metrics();
    let mut rejections = Vec::new();

    if instance.state() != AgentInstanceState::Ready {
        rejections.push(RejectionReason::AgentNotReady(instance.state()));
    }
    if !node.is_ready() {
        rejections.push(RejectionReason::NodeNotReady);
    }
    if !node.is_schedulable() {
        rejections.push(RejectionReason::NodeUnschedulable);
    }
    if node.available_slots() == 0 {
        rejections.push(RejectionReason::NodeAtCapacity);
    }

    let missing_capabilities: Vec<_> = requirements
        .capabilities()
        .difference(definition.spec().capabilities())
        .cloned()
        .collect();
    if !missing_capabilities.is_empty() {
        rejections.push(RejectionReason::MissingCapabilities(missing_capabilities));
    }
    let missing_tools: Vec<_> = requirements
        .tools()
        .difference(definition.spec().tools())
        .cloned()
        .collect();
    if !missing_tools.is_empty() {
        rejections.push(RejectionReason::MissingTools(missing_tools));
    }

    let provider = eligible_provider(task, candidate);
    if provider.is_none() {
        rejections.push(RejectionReason::NoEligibleProvider);
    }

    if let Some(maximum) = effective_latency_limit(task, candidate)
        && metrics.expected_latency() > maximum
    {
        rejections.push(RejectionReason::LatencyExceeded {
            expected: metrics.expected_latency(),
            maximum,
        });
    }
    if let Some(maximum) = effective_cost_limit(task, candidate)
        && metrics.expected_cost_micro_usd() > maximum
    {
        rejections.push(RejectionReason::CostExceeded {
            expected_micro_usd: metrics.expected_cost_micro_usd(),
            maximum_micro_usd: maximum,
        });
    }

    let preferred_roles = requirements.preferred_roles();
    let capability =
        if preferred_roles.is_empty() || preferred_roles.contains(definition.spec().role()) {
            BasisPoints::MAX
        } else {
            BasisPoints::new(5_000).expect("role fallback score is valid")
        };
    let availability_value =
        u32::from(node.available_slots()) * 10_000 / u32::from(node.total_slots());
    let availability =
        BasisPoints::new(availability_value as u16).expect("capacity ratio cannot exceed 10000");

    WorkingEvaluation {
        agent_id: instance.id(),
        node_id: node.id(),
        provider,
        rejections,
        score: None,
        quality: metrics.quality(),
        capability,
        availability,
        latency_millis: metrics.expected_latency().as_millis(),
        cost_micro_usd: metrics.expected_cost_micro_usd(),
    }
}

fn eligible_provider(task: &AgentTask, candidate: &SchedulingCandidate) -> Option<ProviderName> {
    let requirements = task.spec().requirements();
    candidate
        .node()
        .providers()
        .values()
        .filter(|availability| match candidate.definition().spec().model() {
            ModelPolicy::Fixed { provider, .. } => availability.name() == provider,
            ModelPolicy::Auto {
                allowed_providers, ..
            } => allowed_providers.is_empty() || allowed_providers.contains(availability.name()),
        })
        .filter(|availability| {
            requirements.allowed_providers().is_empty()
                || requirements
                    .allowed_providers()
                    .contains(availability.name())
        })
        .filter(|availability| {
            !requirements
                .denied_providers()
                .contains(availability.name())
        })
        .filter(|availability| privacy_allows(requirements.privacy(), availability))
        .map(|availability| availability.name().clone())
        .next()
}

fn privacy_allows(requirement: PrivacyRequirement, availability: &ProviderAvailability) -> bool {
    match requirement {
        PrivacyRequirement::Standard => true,
        PrivacyRequirement::Confidential => availability.accepts_confidential(),
        PrivacyRequirement::LocalOnly => availability.locality() == NodeLocality::Local,
    }
}

fn effective_latency_limit(
    task: &AgentTask,
    candidate: &SchedulingCandidate,
) -> Option<HumanDuration> {
    [
        task.spec().requirements().max_latency(),
        task.spec().budget().timeout(),
        candidate.definition().spec().resources().timeout(),
    ]
    .into_iter()
    .flatten()
    .min()
}

fn effective_cost_limit(task: &AgentTask, candidate: &SchedulingCandidate) -> Option<u64> {
    [
        task.spec().budget().max_cost_micro_usd(),
        candidate
            .definition()
            .spec()
            .resources()
            .max_cost_micro_usd(),
    ]
    .into_iter()
    .flatten()
    .map(|value| value.get())
    .min()
}

fn score_candidate(
    weights: ScoreWeights,
    candidate: &WorkingEvaluation,
    minimum_latency: u128,
    minimum_cost: u64,
) -> Result<ScoreBreakdown, SchedulingError> {
    let latency = ratio_score_u128(minimum_latency, candidate.latency_millis)?;
    let cost_efficiency = if candidate.cost_micro_usd == 0 {
        BasisPoints::MAX
    } else if minimum_cost == 0 {
        BasisPoints::ZERO
    } else {
        ratio_score_u128(
            u128::from(minimum_cost),
            u128::from(candidate.cost_micro_usd),
        )?
    };
    let weighted = u64::from(candidate.quality.get()) * u64::from(weights.quality().get())
        + u64::from(candidate.capability.get()) * u64::from(weights.capability().get())
        + u64::from(candidate.availability.get()) * u64::from(weights.availability().get())
        + u64::from(latency.get()) * u64::from(weights.latency().get())
        + u64::from(cost_efficiency.get()) * u64::from(weights.cost_efficiency().get());
    let total =
        u16::try_from(weighted / 10_000).map_err(|_| SchedulingError::ArithmeticOverflow)?;
    let total = BasisPoints::new(total).ok_or(SchedulingError::ArithmeticOverflow)?;
    Ok(ScoreBreakdown::new(
        candidate.quality,
        candidate.capability,
        candidate.availability,
        latency,
        cost_efficiency,
        total,
    ))
}

fn ratio_score_u128(numerator: u128, denominator: u128) -> Result<BasisPoints, SchedulingError> {
    if denominator == 0 {
        return Ok(BasisPoints::MAX);
    }
    let scaled = numerator
        .checked_mul(10_000)
        .ok_or(SchedulingError::ArithmeticOverflow)?
        / denominator;
    let bounded = scaled.min(10_000);
    let value = u16::try_from(bounded).map_err(|_| SchedulingError::ArithmeticOverflow)?;
    BasisPoints::new(value).ok_or(SchedulingError::ArithmeticOverflow)
}

fn compare_feasible(left: &&WorkingEvaluation, right: &&WorkingEvaluation) -> Ordering {
    let left_score = left.score.expect("feasible candidate has score");
    let right_score = right.score.expect("feasible candidate has score");
    left_score
        .total()
        .cmp(&right_score.total())
        .then_with(|| left_score.quality().cmp(&right_score.quality()))
        .then_with(|| left_score.availability().cmp(&right_score.availability()))
        .then_with(|| right.latency_millis.cmp(&left.latency_millis))
        .then_with(|| right.cost_micro_usd.cmp(&left.cost_micro_usd))
        .then_with(|| right.agent_id.as_uuid().cmp(left.agent_id.as_uuid()))
}

fn finalize_evaluations(working: Vec<WorkingEvaluation>) -> Vec<CandidateEvaluation> {
    working
        .into_iter()
        .map(|evaluation| CandidateEvaluation {
            agent_id: evaluation.agent_id,
            node_id: evaluation.node_id,
            provider: evaluation.provider,
            rejections: evaluation.rejections,
            score: evaluation.score,
        })
        .collect()
}

fn sort_evaluations(evaluations: &mut [CandidateEvaluation]) {
    evaluations.sort_by(|left, right| left.agent_id.as_uuid().cmp(right.agent_id.as_uuid()));
}
