use crate::{ControllerError, ControllerResult, ReconcilePlan, ReconcileState};
use agentkube_agents::{
    AgentCondition, AgentConditionType, AgentDefinition, AgentInstance, AgentInstanceState,
    AgentPhase, AgentStatus, ConditionStatus,
};
use agentkube_core::{AgentId, Resource, ResourceVersion};
use std::collections::HashSet;

/// Reconciles an agent definition's observed phase and conditions.
#[derive(Debug, Clone, Copy, Default)]
pub struct AgentDefinitionReconciler;

impl AgentDefinitionReconciler {
    /// Creates the stateless definition reconciler.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Observes matching runtime instances and produces an idempotent status update.
    pub fn reconcile(
        &self,
        definition: &AgentDefinition,
        instances: &[AgentInstance],
    ) -> ControllerResult<ReconcilePlan<AgentDefinition, ()>> {
        validate_unique_agents(instances)?;
        let matching: Vec<_> = instances
            .iter()
            .filter(|instance| instance.definition_uid() == definition.metadata().uid())
            .collect();
        let current_version = definition.metadata().resource_version();
        let current_status = status_for(current_version, &matching);
        let resource_update = if definition.status() == &current_status {
            None
        } else {
            let next_version = current_version.next()?;
            let mut update = definition.clone();
            *update.status_mut() = status_for(next_version, &matching);
            Some(update)
        };

        let state = if matching
            .iter()
            .any(|instance| is_progressing(instance.state()))
        {
            ReconcileState::Progressing
        } else if matching.is_empty()
            || matching
                .iter()
                .any(|instance| instance.state() == AgentInstanceState::Failed)
            || !matching.iter().any(|instance| is_ready(instance.state()))
        {
            ReconcileState::Blocked
        } else {
            ReconcileState::Converged
        };
        Ok(ReconcilePlan::new(state, resource_update, Vec::new()))
    }
}

fn validate_unique_agents(instances: &[AgentInstance]) -> ControllerResult<()> {
    let mut ids = HashSet::<AgentId>::with_capacity(instances.len());
    for instance in instances {
        if !ids.insert(instance.id()) {
            return Err(ControllerError::DuplicateAgent(instance.id()));
        }
    }
    Ok(())
}

fn status_for(version: ResourceVersion, instances: &[&AgentInstance]) -> AgentStatus {
    let ready = instances
        .iter()
        .filter(|instance| is_ready(instance.state()))
        .count();
    let failed = instances
        .iter()
        .filter(|instance| instance.state() == AgentInstanceState::Failed)
        .count();
    let progressing = instances
        .iter()
        .filter(|instance| is_progressing(instance.state()))
        .count();
    let phase = if failed > 0 {
        AgentPhase::Degraded
    } else if ready > 0 {
        AgentPhase::Ready
    } else {
        AgentPhase::Pending
    };
    let mut status = AgentStatus::pending(version);
    status.set_phase(phase);
    status.set_condition(AgentCondition::new(
        AgentConditionType::Accepted,
        ConditionStatus::True,
        "SpecAccepted",
        "Agent specification passed domain validation",
    ));
    status.set_condition(AgentCondition::new(
        AgentConditionType::DependenciesReady,
        ConditionStatus::True,
        "DependenciesResolved",
        "Agent runtime dependencies are resolved",
    ));
    status.set_condition(AgentCondition::new(
        AgentConditionType::Progressing,
        if progressing > 0 {
            ConditionStatus::True
        } else {
            ConditionStatus::False
        },
        if progressing > 0 {
            "InstancesStarting"
        } else {
            "Stable"
        },
        format!("{progressing} instance(s) are progressing"),
    ));
    status.set_condition(AgentCondition::new(
        AgentConditionType::Available,
        if ready > 0 {
            ConditionStatus::True
        } else {
            ConditionStatus::False
        },
        if ready > 0 {
            "InstancesReady"
        } else {
            "NoReadyInstances"
        },
        format!("{ready} instance(s) are ready"),
    ));
    status
}

const fn is_ready(state: AgentInstanceState) -> bool {
    matches!(
        state,
        AgentInstanceState::Ready | AgentInstanceState::Running | AgentInstanceState::Paused
    )
}

const fn is_progressing(state: AgentInstanceState) -> bool {
    matches!(
        state,
        AgentInstanceState::Pending
            | AgentInstanceState::Scheduling
            | AgentInstanceState::Starting
            | AgentInstanceState::Retrying
    )
}
