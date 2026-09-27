use crate::nodes::NodeRegistry;
use agentkube_agents::{AgentDefinition, AgentDeployment};
use agentkube_queue::TaskQueue;
use agentkube_storage::ResourceRepository;
use agentkube_tasks::AgentTask;
use std::{sync::Arc, time::Instant};

/// Shared dependencies used by all HTTP request handlers.
#[derive(Clone)]
pub struct ApiState {
    agents: Arc<dyn ResourceRepository<AgentDefinition>>,
    deployments: Arc<dyn ResourceRepository<AgentDeployment>>,
    tasks: Arc<dyn ResourceRepository<AgentTask>>,
    queue: Arc<dyn TaskQueue>,
    auth_token: Option<String>,
    nodes: Arc<NodeRegistry>,
    started_at: Instant,
}

impl ApiState {
    /// Creates API state from persistence and queue ports.
    ///
    /// Authentication is disabled and the node registry starts empty; use
    /// [`ApiState::with_auth_token`] to enforce Bearer authentication.
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
            auth_token: None,
            nodes: Arc::new(NodeRegistry::new()),
            started_at: Instant::now(),
        }
    }

    /// Enforces Bearer authentication on versioned endpoints.
    #[must_use]
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Returns the required Bearer token, if authentication is enforced.
    ///
    /// The value is exposed for request comparison only and never logged.
    #[must_use]
    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    /// Returns the shared live-node registry.
    #[must_use]
    pub fn node_registry(&self) -> Arc<NodeRegistry> {
        Arc::clone(&self.nodes)
    }

    /// Returns seconds since this state (and process) started.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
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
