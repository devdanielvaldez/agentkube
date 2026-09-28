//! Agent-status reconciliation tick.
//!
//! Each cycle runs the pure [`AgentDefinitionReconciler`] over every stored
//! definition with live instance snapshots and persists status updates
//! through optimistic-concurrency replaces. Deployment instance management
//! (spawning and deleting replicas) is not wired yet: the tick reports
//! deployment drift visibly instead of pretending to converge it.

use crate::OperatorError;
use agentkube_agents::AgentDeployment;
use agentkube_controllers::{AgentDefinitionReconciler, ReconcileState};
use agentkube_core::Resource;
use agentkube_storage::ResourceRepository;
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
    /// Deployments whose desired replicas lack instance management.
    pub unmanaged: Vec<String>,
}

/// Reconciles agent statuses once; reports (never executes) deployment drift.
pub async fn reconcile_once(
    agents: &Arc<dyn ResourceRepository<agentkube_agents::AgentDefinition>>,
    deployments: &Arc<dyn ResourceRepository<AgentDeployment>>,
    state: &RepositoryWorkerStateStore,
) -> Result<ReconcileSummary, OperatorError> {
    let definitions = agents.list(None).await?;
    let instances = state
        .instances()
        .map_err(|error| OperatorError::Worker(error.to_string()))?;
    let reconciler = AgentDefinitionReconciler::new();
    let mut summary = ReconcileSummary::default();
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
    for deployment in deployments.list(None).await? {
        summary
            .unmanaged
            .push(deployment.metadata().name().as_str().to_owned());
    }
    summary.unmanaged.sort();
    summary.unmanaged.dedup();
    Ok(summary)
}
