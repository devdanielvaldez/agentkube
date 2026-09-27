use crate::{ControllerError, ControllerResult, ReconcilePlan, ReconcileState};
use agentkube_agents::{
    AgentDeployment, AgentDeploymentStatus, AgentInstance, AgentInstanceState, AgentSpec,
    ReplicaCount, RestartPolicy,
};
use agentkube_core::{AgentId, Resource, ResourceUid};
use std::collections::HashSet;

/// Agent instance annotated with deployment ownership and its applied template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedAgentInstance {
    owner_uid: ResourceUid,
    applied_template: AgentSpec,
    instance: AgentInstance,
}

impl ManagedAgentInstance {
    /// Creates a controller snapshot for one managed replica.
    #[must_use]
    pub const fn new(
        owner_uid: ResourceUid,
        applied_template: AgentSpec,
        instance: AgentInstance,
    ) -> Self {
        Self {
            owner_uid,
            applied_template,
            instance,
        }
    }

    /// Owning deployment UID.
    #[must_use]
    pub const fn owner_uid(&self) -> ResourceUid {
        self.owner_uid
    }

    /// Template installed when this replica was created or replaced.
    #[must_use]
    pub const fn applied_template(&self) -> &AgentSpec {
        &self.applied_template
    }

    /// Runtime instance snapshot.
    #[must_use]
    pub const fn instance(&self) -> &AgentInstance {
        &self.instance
    }
}

/// Ordered side effect emitted by the deployment reconciler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentAction {
    /// Create one pending replica from the current template.
    CreateReplica {
        /// Deployment receiving ownership.
        owner_uid: ResourceUid,
        /// Template to apply.
        template: AgentSpec,
    },
    /// Delete one replica using optimistic concurrency.
    DeleteReplica {
        /// Replica identifier.
        agent_id: AgentId,
        /// Revision observed by this reconciliation cycle.
        expected_revision: u64,
    },
    /// Restart a failed replica without changing its template.
    RestartReplica {
        /// Replica identifier.
        agent_id: AgentId,
        /// Revision observed by this reconciliation cycle.
        expected_revision: u64,
    },
    /// Replace a safe, outdated or completed replica with the current template.
    ReplaceReplica {
        /// Replica identifier being replaced.
        agent_id: AgentId,
        /// Revision observed by this reconciliation cycle.
        expected_revision: u64,
        /// Current deployment template.
        template: AgentSpec,
    },
}

/// Reconciles `AgentDeployment` desired replicas against runtime instances.
#[derive(Debug, Clone, Copy, Default)]
pub struct AgentDeploymentReconciler;

impl AgentDeploymentReconciler {
    /// Creates the stateless deployment reconciler.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Produces an idempotent action plan from one consistent snapshot.
    pub fn reconcile(
        &self,
        deployment: &AgentDeployment,
        instances: &[ManagedAgentInstance],
    ) -> ControllerResult<ReconcilePlan<AgentDeployment, DeploymentAction>> {
        validate_unique_agents(instances)?;
        let owner_uid = deployment.metadata().uid();
        let desired = deployment.spec().replicas().get() as usize;
        let template = deployment.spec().template();
        let mut owned: Vec<_> = instances
            .iter()
            .filter(|managed| managed.owner_uid() == owner_uid)
            .collect();
        owned.sort_by(|left, right| {
            deletion_priority(left.instance().state())
                .cmp(&deletion_priority(right.instance().state()))
                .then_with(|| {
                    left.instance()
                        .id()
                        .as_uuid()
                        .cmp(right.instance().id().as_uuid())
                })
        });

        let mut actions = Vec::new();
        let mut selected = HashSet::new();

        for managed in &owned {
            if managed.instance().state() == AgentInstanceState::Terminated {
                push_delete(&mut actions, &mut selected, managed.instance());
            }
        }

        let slot_count = owned
            .iter()
            .filter(|managed| managed.instance().state() != AgentInstanceState::Terminated)
            .count();
        for managed in owned
            .iter()
            .filter(|managed| managed.instance().state() != AgentInstanceState::Terminated)
            .take(slot_count.saturating_sub(desired))
        {
            push_delete(&mut actions, &mut selected, managed.instance());
        }

        for managed in owned.iter().filter(|managed| {
            managed.instance().state() != AgentInstanceState::Terminated
                && !selected.contains(&managed.instance().id())
        }) {
            let instance = managed.instance();
            let outdated = managed.applied_template() != template;
            if outdated {
                if is_safe_to_replace(instance.state()) {
                    actions.push(DeploymentAction::ReplaceReplica {
                        agent_id: instance.id(),
                        expected_revision: instance.revision(),
                        template: template.clone(),
                    });
                }
                continue;
            }

            match (instance.state(), deployment.spec().restart_policy()) {
                (AgentInstanceState::Failed, RestartPolicy::OnFailure | RestartPolicy::Always) => {
                    actions.push(DeploymentAction::RestartReplica {
                        agent_id: instance.id(),
                        expected_revision: instance.revision(),
                    });
                }
                (AgentInstanceState::Completed, RestartPolicy::Always) => {
                    actions.push(DeploymentAction::ReplaceReplica {
                        agent_id: instance.id(),
                        expected_revision: instance.revision(),
                        template: template.clone(),
                    });
                }
                _ => {}
            }
        }

        for _ in 0..desired.saturating_sub(slot_count) {
            actions.push(DeploymentAction::CreateReplica {
                owner_uid,
                template: template.clone(),
            });
        }

        let status = observed_status(deployment, &owned)?;
        let resource_update = deployment_update(deployment, status)?;
        let ready = owned
            .iter()
            .filter(|managed| is_ready(managed.instance().state()))
            .count()
            .min(desired);
        let updated = owned
            .iter()
            .filter(|managed| {
                managed.instance().state() != AgentInstanceState::Terminated
                    && managed.applied_template() == template
            })
            .count()
            .min(desired);
        let unsafe_outdated = owned.iter().any(|managed| {
            managed.applied_template() != template
                && !is_safe_to_replace(managed.instance().state())
        });
        let state = if !actions.is_empty() {
            ReconcileState::Progressing
        } else if ready == desired && updated == desired {
            ReconcileState::Converged
        } else if unsafe_outdated {
            ReconcileState::Progressing
        } else {
            ReconcileState::Blocked
        };

        Ok(ReconcilePlan::new(state, resource_update, actions))
    }
}

fn validate_unique_agents(instances: &[ManagedAgentInstance]) -> ControllerResult<()> {
    let mut ids = HashSet::with_capacity(instances.len());
    for managed in instances {
        if !ids.insert(managed.instance().id()) {
            return Err(ControllerError::DuplicateAgent(managed.instance().id()));
        }
    }
    Ok(())
}

fn push_delete(
    actions: &mut Vec<DeploymentAction>,
    selected: &mut HashSet<AgentId>,
    instance: &AgentInstance,
) {
    if selected.insert(instance.id()) {
        actions.push(DeploymentAction::DeleteReplica {
            agent_id: instance.id(),
            expected_revision: instance.revision(),
        });
    }
}

const fn deletion_priority(state: AgentInstanceState) -> u8 {
    match state {
        AgentInstanceState::Terminated => 0,
        AgentInstanceState::Completed => 1,
        AgentInstanceState::Failed => 2,
        AgentInstanceState::Pending => 3,
        AgentInstanceState::Scheduling => 4,
        AgentInstanceState::Retrying | AgentInstanceState::Starting => 5,
        AgentInstanceState::Paused => 6,
        AgentInstanceState::Ready => 7,
        AgentInstanceState::Running => 8,
    }
}

const fn is_safe_to_replace(state: AgentInstanceState) -> bool {
    !matches!(
        state,
        AgentInstanceState::Running | AgentInstanceState::Paused
    )
}

const fn is_ready(state: AgentInstanceState) -> bool {
    matches!(
        state,
        AgentInstanceState::Ready | AgentInstanceState::Running | AgentInstanceState::Paused
    )
}

fn observed_status(
    deployment: &AgentDeployment,
    owned: &[&ManagedAgentInstance],
) -> ControllerResult<AgentDeploymentStatus> {
    let desired = deployment.spec().replicas().get();
    let bounded_count = |predicate: fn(&ManagedAgentInstance) -> bool| {
        let count = owned
            .iter()
            .filter(|managed| predicate(managed))
            .count()
            .min(desired as usize);
        u32::try_from(count).map_err(|_| ControllerError::ReplicaCountOverflow)
    };
    let ready = bounded_count(|managed| is_ready(managed.instance().state()))?;
    let available =
        bounded_count(|managed| managed.instance().state() == AgentInstanceState::Ready)?;
    let template = deployment.spec().template();
    let updated = owned
        .iter()
        .filter(|managed| {
            managed.instance().state() != AgentInstanceState::Terminated
                && managed.applied_template() == template
        })
        .count()
        .min(desired as usize);
    let updated = u32::try_from(updated).map_err(|_| ControllerError::ReplicaCountOverflow)?;
    let observed_version = if deployment.status().observed_version()
        == deployment.metadata().resource_version()
        && deployment.status().desired_replicas() == deployment.spec().replicas()
        && deployment.status().ready_replicas().get() == ready
        && deployment.status().available_replicas().get() == available
        && deployment.status().updated_replicas().get() == updated
    {
        deployment.metadata().resource_version()
    } else {
        deployment.metadata().resource_version().next()?
    };
    AgentDeploymentStatus::new(
        observed_version,
        deployment.spec().replicas(),
        ReplicaCount::new(ready).map_err(|_| ControllerError::ReplicaCountOverflow)?,
        ReplicaCount::new(available).map_err(|_| ControllerError::ReplicaCountOverflow)?,
        ReplicaCount::new(updated).map_err(|_| ControllerError::ReplicaCountOverflow)?,
    )
    .map_err(Into::into)
}

fn deployment_update(
    deployment: &AgentDeployment,
    status: AgentDeploymentStatus,
) -> ControllerResult<Option<AgentDeployment>> {
    if deployment.status() == &status {
        return Ok(None);
    }
    let mut update = deployment.clone();
    update.set_status(status)?;
    Ok(Some(update))
}
