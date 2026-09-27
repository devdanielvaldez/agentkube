use agentkube_agents::{AgentDefinition, AgentDeployment};
use agentkube_queue::TaskQueue;
use agentkube_storage::ResourceRepository;
use agentkube_tasks::AgentTask;
use std::sync::Arc;

/// Shared dependencies used by all HTTP request handlers.
#[derive(Clone)]
pub struct ApiState {
    agents: Arc<dyn ResourceRepository<AgentDefinition>>,
    deployments: Arc<dyn ResourceRepository<AgentDeployment>>,
    tasks: Arc<dyn ResourceRepository<AgentTask>>,
    queue: Arc<dyn TaskQueue>,
}

impl ApiState {
    /// Creates API state from persistence and queue ports.
    #[must_use]
    pub fn new(
        agents: Arc<dyn ResourceRepository<AgentDefinition>>,
        deployments: Arc<dyn ResourceRepository<AgentDeployment>>,
        tasks: Arc<dyn ResourceRepository<AgentTask>>,
        queue: Arc<dyn TaskQueue>,
    ) -> Self {
        Self {
            agents,
            deployments,
            tasks,
            queue,
        }
    }

    pub(crate) fn agents(&self) -> &dyn ResourceRepository<AgentDefinition> {
        self.agents.as_ref()
    }

    pub(crate) fn deployments(&self) -> &dyn ResourceRepository<AgentDeployment> {
        self.deployments.as_ref()
    }

    pub(crate) fn tasks(&self) -> &dyn ResourceRepository<AgentTask> {
        self.tasks.as_ref()
    }

    pub(crate) fn queue(&self) -> &dyn TaskQueue {
        self.queue.as_ref()
    }
}
