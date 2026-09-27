use crate::{AgentRole, Capability, Instructions, ModelPolicy, ToolName};
use agentkube_core::HumanDuration;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, num::NonZeroU64};

/// Desired behavior and execution boundaries of an agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSpec {
    role: AgentRole,
    model: ModelPolicy,
    instructions: Instructions,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    capabilities: BTreeSet<Capability>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    tools: BTreeSet<ToolName>,
    #[serde(default)]
    resources: AgentResourceLimits,
    #[serde(default)]
    permissions: AgentPermissions,
    #[serde(default)]
    memory: MemoryPolicy,
}

impl AgentSpec {
    /// Creates a minimal, secure-by-default agent specification.
    #[must_use]
    pub fn new(role: AgentRole, model: ModelPolicy, instructions: Instructions) -> Self {
        Self {
            role,
            model,
            instructions,
            capabilities: BTreeSet::new(),
            tools: BTreeSet::new(),
            resources: AgentResourceLimits::default(),
            permissions: AgentPermissions::default(),
            memory: MemoryPolicy::default(),
        }
    }

    /// Adds a scheduler-visible capability.
    #[must_use]
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    /// Grants access to a named tool, subject to policy enforcement.
    #[must_use]
    pub fn with_tool(mut self, tool: ToolName) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Applies resource limits.
    #[must_use]
    pub fn with_resources(mut self, resources: AgentResourceLimits) -> Self {
        self.resources = resources;
        self
    }

    /// Applies execution permissions.
    #[must_use]
    pub const fn with_permissions(mut self, permissions: AgentPermissions) -> Self {
        self.permissions = permissions;
        self
    }

    /// Applies the agent memory policy.
    #[must_use]
    pub const fn with_memory(mut self, memory: MemoryPolicy) -> Self {
        self.memory = memory;
        self
    }

    /// Returns the agent role.
    #[must_use]
    pub const fn role(&self) -> &AgentRole {
        &self.role
    }

    /// Returns the model-selection policy.
    #[must_use]
    pub const fn model(&self) -> &ModelPolicy {
        &self.model
    }

    /// Returns the model instructions.
    #[must_use]
    pub const fn instructions(&self) -> &Instructions {
        &self.instructions
    }

    /// Returns scheduler-visible capabilities.
    #[must_use]
    pub const fn capabilities(&self) -> &BTreeSet<Capability> {
        &self.capabilities
    }

    /// Returns requested tools.
    #[must_use]
    pub const fn tools(&self) -> &BTreeSet<ToolName> {
        &self.tools
    }

    /// Returns resource limits.
    #[must_use]
    pub const fn resources(&self) -> &AgentResourceLimits {
        &self.resources
    }

    /// Returns execution permissions.
    #[must_use]
    pub const fn permissions(&self) -> &AgentPermissions {
        &self.permissions
    }

    /// Returns the memory policy.
    #[must_use]
    pub const fn memory(&self) -> &MemoryPolicy {
        &self.memory
    }
}

/// Token, cost, and runtime bounds for one agent execution.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tokens: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_cost_micro_usd: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<HumanDuration>,
}

impl AgentResourceLimits {
    /// Creates unbounded limits. Cluster policy may still impose ceilings.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_tokens: None,
            max_cost_micro_usd: None,
            timeout: None,
        }
    }

    /// Sets the maximum number of generated and consumed tokens.
    #[must_use]
    pub const fn with_max_tokens(mut self, value: NonZeroU64) -> Self {
        self.max_tokens = Some(value);
        self
    }

    /// Sets the maximum cost in millionths of one US dollar.
    #[must_use]
    pub const fn with_max_cost_micro_usd(mut self, value: NonZeroU64) -> Self {
        self.max_cost_micro_usd = Some(value);
        self
    }

    /// Sets the execution timeout.
    #[must_use]
    pub const fn with_timeout(mut self, value: HumanDuration) -> Self {
        self.timeout = Some(value);
        self
    }

    /// Returns the token ceiling.
    #[must_use]
    pub const fn max_tokens(&self) -> Option<NonZeroU64> {
        self.max_tokens
    }

    /// Returns the financial ceiling in millionths of one US dollar.
    #[must_use]
    pub const fn max_cost_micro_usd(&self) -> Option<NonZeroU64> {
        self.max_cost_micro_usd
    }

    /// Returns the execution timeout.
    #[must_use]
    pub const fn timeout(&self) -> Option<HumanDuration> {
        self.timeout
    }
}

/// Network access granted to an agent sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkAccess {
    /// No network access.
    #[default]
    Disabled,
    /// Only policy-approved destinations are reachable.
    Restricted,
    /// General outbound access is allowed.
    Unrestricted,
}

/// Filesystem access granted to an agent sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilesystemAccess {
    /// No mounted filesystem.
    #[default]
    None,
    /// Workspace is mounted read-only.
    ReadOnly,
    /// Workspace is mounted read-write.
    Workspace,
}

/// Security-sensitive execution capabilities granted to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPermissions {
    network: NetworkAccess,
    filesystem: FilesystemAccess,
    shell: bool,
    production: bool,
}

impl AgentPermissions {
    /// Creates permissions with every capability denied.
    #[must_use]
    pub const fn denied() -> Self {
        Self {
            network: NetworkAccess::Disabled,
            filesystem: FilesystemAccess::None,
            shell: false,
            production: false,
        }
    }

    /// Sets network access.
    #[must_use]
    pub const fn with_network(mut self, access: NetworkAccess) -> Self {
        self.network = access;
        self
    }

    /// Sets filesystem access.
    #[must_use]
    pub const fn with_filesystem(mut self, access: FilesystemAccess) -> Self {
        self.filesystem = access;
        self
    }

    /// Enables or disables shell execution.
    #[must_use]
    pub const fn with_shell(mut self, enabled: bool) -> Self {
        self.shell = enabled;
        self
    }

    /// Enables or disables access to production capabilities.
    #[must_use]
    pub const fn with_production(mut self, enabled: bool) -> Self {
        self.production = enabled;
        self
    }

    /// Returns network access.
    #[must_use]
    pub const fn network(self) -> NetworkAccess {
        self.network
    }

    /// Returns filesystem access.
    #[must_use]
    pub const fn filesystem(self) -> FilesystemAccess {
        self.filesystem
    }

    /// Returns whether shell execution is allowed.
    #[must_use]
    pub const fn shell(self) -> bool {
        self.shell
    }

    /// Returns whether production capabilities are allowed.
    #[must_use]
    pub const fn production(self) -> bool {
        self.production
    }
}

/// Memory lifecycle assigned to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MemoryPolicy {
    /// No state is retained between model calls.
    #[default]
    Disabled,
    /// Memory is retained for a bounded duration.
    Ephemeral {
        /// Time-to-live for ephemeral state.
        ttl: HumanDuration,
    },
    /// State is retained by a configured persistent-memory adapter.
    Persistent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelName, ProviderName};

    fn spec() -> AgentSpec {
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("openai").unwrap(),
                ModelName::new("gpt-5").unwrap(),
            ),
            Instructions::new("Build reliable software.").unwrap(),
        )
    }

    #[test]
    fn specification_is_secure_by_default() {
        let spec = spec();

        assert_eq!(spec.permissions(), &AgentPermissions::denied());
        assert_eq!(spec.memory(), &MemoryPolicy::Disabled);
        assert!(spec.tools().is_empty());
    }

    #[test]
    fn capabilities_and_tools_are_deduplicated() {
        let capability = Capability::new("coding").unwrap();
        let tool = ToolName::new("github").unwrap();
        let spec = spec()
            .with_capability(capability.clone())
            .with_capability(capability)
            .with_tool(tool.clone())
            .with_tool(tool);

        assert_eq!(spec.capabilities().len(), 1);
        assert_eq!(spec.tools().len(), 1);
    }
}
