use agentkube_agents::{AgentRole, Capability, ProviderName, ToolName};
use agentkube_core::HumanDuration;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeSet, error::Error, fmt};

/// Data-locality requirement considered during scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrivacyRequirement {
    /// Cloud and local providers may be considered.
    #[default]
    Standard,
    /// Only providers approved for confidential data may be considered.
    Confidential,
    /// The task must remain on local infrastructure and models.
    LocalOnly,
}

/// Constraints an agent and its execution environment must satisfy.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRequirements {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    capabilities: BTreeSet<Capability>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    tools: BTreeSet<ToolName>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    preferred_roles: BTreeSet<AgentRole>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    allowed_providers: BTreeSet<ProviderName>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    denied_providers: BTreeSet<ProviderName>,
    #[serde(default)]
    privacy: PrivacyRequirement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_latency: Option<HumanDuration>,
}

impl TaskRequirements {
    /// Creates requirements with no capability or placement constraints.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requires a capability.
    #[must_use]
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    /// Requires a tool.
    #[must_use]
    pub fn with_tool(mut self, tool: ToolName) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Prefers agents with the supplied role.
    #[must_use]
    pub fn with_preferred_role(mut self, role: AgentRole) -> Self {
        self.preferred_roles.insert(role);
        self
    }

    /// Allows a provider, rejecting contradictory placement policy.
    pub fn allow_provider(mut self, provider: ProviderName) -> Result<Self, TaskRequirementsError> {
        if self.denied_providers.contains(&provider) {
            return Err(TaskRequirementsError::ProviderBothAllowedAndDenied(
                provider,
            ));
        }
        self.allowed_providers.insert(provider);
        Ok(self)
    }

    /// Denies a provider, rejecting contradictory placement policy.
    pub fn deny_provider(mut self, provider: ProviderName) -> Result<Self, TaskRequirementsError> {
        if self.allowed_providers.contains(&provider) {
            return Err(TaskRequirementsError::ProviderBothAllowedAndDenied(
                provider,
            ));
        }
        self.denied_providers.insert(provider);
        Ok(self)
    }

    /// Sets the data-locality requirement.
    #[must_use]
    pub const fn with_privacy(mut self, privacy: PrivacyRequirement) -> Self {
        self.privacy = privacy;
        self
    }

    /// Sets the maximum acceptable execution latency.
    #[must_use]
    pub const fn with_max_latency(mut self, latency: HumanDuration) -> Self {
        self.max_latency = Some(latency);
        self
    }

    /// Validates cross-field placement constraints.
    pub fn validate(&self) -> Result<(), TaskRequirementsError> {
        if let Some(provider) = self
            .allowed_providers
            .intersection(&self.denied_providers)
            .next()
        {
            Err(TaskRequirementsError::ProviderBothAllowedAndDenied(
                provider.clone(),
            ))
        } else {
            Ok(())
        }
    }

    /// Returns required capabilities.
    #[must_use]
    pub const fn capabilities(&self) -> &BTreeSet<Capability> {
        &self.capabilities
    }

    /// Returns required tools.
    #[must_use]
    pub const fn tools(&self) -> &BTreeSet<ToolName> {
        &self.tools
    }

    /// Returns preferred agent roles.
    #[must_use]
    pub const fn preferred_roles(&self) -> &BTreeSet<AgentRole> {
        &self.preferred_roles
    }

    /// Returns the provider allowlist. Empty means every non-denied provider.
    #[must_use]
    pub const fn allowed_providers(&self) -> &BTreeSet<ProviderName> {
        &self.allowed_providers
    }

    /// Returns explicitly denied providers.
    #[must_use]
    pub const fn denied_providers(&self) -> &BTreeSet<ProviderName> {
        &self.denied_providers
    }

    /// Returns the privacy requirement.
    #[must_use]
    pub const fn privacy(&self) -> PrivacyRequirement {
        self.privacy
    }

    /// Returns the latency ceiling.
    #[must_use]
    pub const fn max_latency(&self) -> Option<HumanDuration> {
        self.max_latency
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskRequirementsWire {
    #[serde(default)]
    capabilities: BTreeSet<Capability>,
    #[serde(default)]
    tools: BTreeSet<ToolName>,
    #[serde(default)]
    preferred_roles: BTreeSet<AgentRole>,
    #[serde(default)]
    allowed_providers: BTreeSet<ProviderName>,
    #[serde(default)]
    denied_providers: BTreeSet<ProviderName>,
    #[serde(default)]
    privacy: PrivacyRequirement,
    #[serde(default)]
    max_latency: Option<HumanDuration>,
}

impl<'de> Deserialize<'de> for TaskRequirements {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TaskRequirementsWire::deserialize(deserializer)?;
        let requirements = Self {
            capabilities: wire.capabilities,
            tools: wire.tools,
            preferred_roles: wire.preferred_roles,
            allowed_providers: wire.allowed_providers,
            denied_providers: wire.denied_providers,
            privacy: wire.privacy,
            max_latency: wire.max_latency,
        };
        requirements.validate().map_err(de::Error::custom)?;
        Ok(requirements)
    }
}

/// Contradictory task placement requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRequirementsError {
    /// One provider appears in both allow and deny sets.
    ProviderBothAllowedAndDenied(ProviderName),
}

impl fmt::Display for TaskRequirementsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderBothAllowedAndDenied(provider) => write!(
                formatter,
                "provider {provider} cannot be both allowed and denied"
            ),
        }
    }
}

impl Error for TaskRequirementsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contradictory_provider_policy_is_rejected() {
        let provider = ProviderName::new("openai").unwrap();
        let result = TaskRequirements::new()
            .allow_provider(provider.clone())
            .unwrap()
            .deny_provider(provider);

        assert!(matches!(
            result,
            Err(TaskRequirementsError::ProviderBothAllowedAndDenied(_))
        ));
    }

    #[test]
    fn deserialization_preserves_provider_invariant() {
        let json = r#"{"allowedProviders":["openai"],"deniedProviders":["openai"]}"#;
        assert!(serde_json::from_str::<TaskRequirements>(json).is_err());
    }
}
