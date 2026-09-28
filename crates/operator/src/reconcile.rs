//! Agent-status reconciliation tick.
//!
//! Each cycle converges deployment-owned instances on the embedded node, then
//! runs the pure [`AgentDefinitionReconciler`] over every stored definition
//! and persists status through optimistic-concurrency replaces.

use crate::OperatorError;
use agentkube_agents::{
    AgentDefinition, AgentDeployment, AgentDeploymentStatus, AgentInstanceState, ReplicaCount,
};
use agentkube_controllers::{AgentDefinitionReconciler, ReconcileState};
use agentkube_core::{Metadata, NodeId, Resource};
use agentkube_storage::{ResourceKey, ResourceRepository};
use agentkube_workers::RepositoryWorkerStateStore;
use std::sync::Arc;

/// Counts from one reconciliation cycle.
///
/// The binary logs `updated` names as they happen and diffs `blocked` and
/// `unmanaged` against the previous cycle so steady state stays quiet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileSummary {
    /// Agents whose status was persisted this cycle.
    pub updated: Vec<String>,
    /// Agents not converged this cycle.
    pub blocked: Vec<String>,
}

/// Reconciles deployment replicas and agent statuses once.
pub async fn reconcile_once(
    agents: &Arc<dyn ResourceRepository<agentkube_agents::AgentDefinition>>,
    deployments: &Arc<dyn ResourceRepository<AgentDeployment>>,
    state: &RepositoryWorkerStateStore,
    node_id: NodeId,
) -> Result<ReconcileSummary, OperatorError> {
    let mut summary = ReconcileSummary::default();
    for mut deployment in deployments.list(None).await? {
        let child_name = format!("agentkube-deployment-{}", deployment.metadata().uid());
        let child_metadata =
            Metadata::in_namespace(child_name, deployment.metadata().namespace().clone())
                .map_err(|error| OperatorError::Reconcile(error.to_string()))?;
        let child_key = ResourceKey::from(&child_metadata);
        let definition = match agents.get(&child_key).await? {
            Some(mut current) => {
                if current.spec() != deployment.spec().template() {
                    *current.spec_mut() = deployment.spec().template().clone();
                    agents.replace(current).await?
                } else {
                    current
                }
            }
            None => {
                agents
                    .create(AgentDefinition::new(
                        child_metadata,
                        deployment.spec().template().clone(),
                    ))
                    .await?
            }
        };
        let desired = deployment.spec().replicas().get() as usize;
        let instances = state
            .reconcile_instances(&definition, node_id, desired)
            .map_err(|error| OperatorError::Worker(error.to_string()))?;
        let ready = instances
            .iter()
            .filter(|instance| {
                matches!(
                    instance.state(),
                    AgentInstanceState::Ready
                        | AgentInstanceState::Running
                        | AgentInstanceState::Paused
                )
            })
            .count()
            .min(desired);
        let available = instances
            .iter()
            .filter(|instance| instance.state() == AgentInstanceState::Ready)
            .count()
            .min(desired);
        let updated = instances.len().min(desired);
        let desired_replicas = deployment.spec().replicas();
        let ready_replicas = replica_count(ready)?;
        let available_replicas = replica_count(available)?;
        let updated_replicas = replica_count(updated)?;
        let status_is_current = deployment.status().observed_version()
            == deployment.metadata().resource_version()
            && deployment.status().desired_replicas() == desired_replicas
            && deployment.status().ready_replicas() == ready_replicas
            && deployment.status().available_replicas() == available_replicas
            && deployment.status().updated_replicas() == updated_replicas;
        if !status_is_current {
            let observed_version = deployment
                .metadata()
                .resource_version()
                .next()
                .map_err(|error| OperatorError::Reconcile(error.to_string()))?;
            deployment
                .set_status(
                    AgentDeploymentStatus::new(
                        observed_version,
                        desired_replicas,
                        ready_replicas,
                        available_replicas,
                        updated_replicas,
                    )
                    .map_err(|error| OperatorError::Reconcile(error.to_string()))?,
                )
                .map_err(|error| OperatorError::Reconcile(error.to_string()))?;
            deployments.replace(deployment).await?;
        }
    }

    let definitions = agents.list(None).await?;
    let instances = state
        .instances()
        .map_err(|error| OperatorError::Worker(error.to_string()))?;
    let reconciler = AgentDefinitionReconciler::new();
    for definition in &definitions {
        let name = definition.metadata().name().as_str().to_owned();
        let plan = reconciler
            .reconcile(definition, &instances)
            .map_err(|error| OperatorError::Reconcile(error.to_string()))?;
        match plan.state() {
            ReconcileState::Converged => {}
            ReconcileState::Progressing | ReconcileState::Blocked => {
                summary.blocked.push(name.clone());
            }
        }
        if let Some(update) = plan.into_resource_update() {
            agents.replace(update).await?;
            summary.updated.push(name.clone());
        }
    }
    Ok(summary)
}

fn replica_count(value: usize) -> Result<ReplicaCount, OperatorError> {
    let value = u32::try_from(value)
        .map_err(|_| OperatorError::Reconcile("replica count exceeds u32".to_owned()))?;
    ReplicaCount::new(value).map_err(|error| OperatorError::Reconcile(error.to_string()))
}
