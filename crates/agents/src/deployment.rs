use crate::definition::{agent_type_meta, validate_type_meta};
use crate::{AgentDocumentError, AgentRole, AgentSpec, Capability};
use agentkube_core::{Metadata, Resource, ResourceVersion};
use agentkube_protocol::ResourceDocument;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeSet, error::Error, fmt};

const MAX_REPLICAS: u32 = 10_000;

/// Desired replica count, bounded to protect the control plane from accidents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ReplicaCount(u32);

impl ReplicaCount {
    /// Creates a replica count in the inclusive range `0..=10_000`.
    pub const fn new(value: u32) -> Result<Self, ReplicaCountError> {
        if value <= MAX_REPLICAS {
            Ok(Self(value))
        } else {
            Err(ReplicaCountError(value))
        }
    }

    /// Returns the numeric replica count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReplicaCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Replica count exceeds the control-plane safety ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplicaCountError(u32);

impl ReplicaCountError {
    /// Returns the rejected count.
    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ReplicaCountError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "replica count {} exceeds {MAX_REPLICAS}", self.0)
    }
}

impl Error for ReplicaCountError {}

/// Restart behavior applied when an agent instance exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestartPolicy {
    /// Never replace an exited instance.
    Never,
    /// Replace only failed instances.
    #[default]
    OnFailure,
    /// Replace completed and failed instances.
    Always,
}

/// Capability-based selector used to match an agent template.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSelector {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    roles: BTreeSet<AgentRole>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    capabilities: BTreeSet<Capability>,
}

impl AgentSelector {
    /// Creates a selector that matches any agent specification.
    #[must_use]
    pub const fn any() -> Self {
        Self {
            roles: BTreeSet::new(),
            capabilities: BTreeSet::new(),
        }
    }

    /// Requires one of the supplied roles.
    #[must_use]
    pub fn with_role(mut self, role: AgentRole) -> Self {
        self.roles.insert(role);
        self
    }

    /// Requires an advertised capability.
    #[must_use]
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    /// Returns whether an agent specification satisfies every constraint.
    #[must_use]
    pub fn matches(&self, spec: &AgentSpec) -> bool {
        (self.roles.is_empty() || self.roles.contains(spec.role()))
            && self.capabilities.is_subset(spec.capabilities())
    }

    /// Returns accepted roles. An empty set accepts every role.
    #[must_use]
    pub const fn roles(&self) -> &BTreeSet<AgentRole> {
        &self.roles
    }

    /// Returns required capabilities.
    #[must_use]
    pub const fn capabilities(&self) -> &BTreeSet<Capability> {
        &self.capabilities
    }
}

/// Desired state maintained by an agent deployment controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDeploymentSpec {
    replicas: ReplicaCount,
    #[serde(default)]
    selector: AgentSelector,
    template: AgentSpec,
    #[serde(default)]
    restart_policy: RestartPolicy,
}

impl AgentDeploymentSpec {
    /// Creates a deployment specification.
    #[must_use]
    pub const fn new(replicas: ReplicaCount, template: AgentSpec) -> Self {
        Self {
            replicas,
            selector: AgentSelector::any(),
            template,
            restart_policy: RestartPolicy::OnFailure,
        }
    }

    /// Applies a selector that the template must satisfy.
    #[must_use]
    pub fn with_selector(mut self, selector: AgentSelector) -> Self {
        self.selector = selector;
        self
    }

    /// Applies instance restart behavior.
    #[must_use]
    pub const fn with_restart_policy(mut self, policy: RestartPolicy) -> Self {
        self.restart_policy = policy;
        self
    }

    /// Returns the desired replica count.
    #[must_use]
    pub const fn replicas(&self) -> ReplicaCount {
        self.replicas
    }

    /// Returns the agent selector.
    #[must_use]
    pub const fn selector(&self) -> &AgentSelector {
        &self.selector
    }

    /// Returns the agent template.
    #[must_use]
    pub const fn template(&self) -> &AgentSpec {
        &self.template
    }

    /// Returns the restart policy.
    #[must_use]
    pub const fn restart_policy(&self) -> RestartPolicy {
        self.restart_policy
    }

    /// Changes the desired replica count.
    pub fn scale(&mut self, replicas: ReplicaCount) {
        self.replicas = replicas;
    }

    /// Validates internal selector/template consistency.
    pub fn validate(&self) -> Result<(), DeploymentStatusError> {
        if self.selector.matches(&self.template) {
            Ok(())
        } else {
            Err(DeploymentStatusError::SelectorDoesNotMatchTemplate)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentDeploymentSpecWire {
    replicas: ReplicaCount,
    #[serde(default)]
    selector: AgentSelector,
    template: AgentSpec,
    #[serde(default)]
    restart_policy: RestartPolicy,
}

impl<'de> Deserialize<'de> for AgentDeploymentSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AgentDeploymentSpecWire::deserialize(deserializer)?;
        let spec = Self {
            replicas: wire.replicas,
            selector: wire.selector,
            template: wire.template,
            restart_policy: wire.restart_policy,
        };
        spec.validate().map_err(de::Error::custom)?;
        Ok(spec)
    }
}

/// Observed replica counts for an agent deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDeploymentStatus {
    observed_version: ResourceVersion,
    desired_replicas: ReplicaCount,
    ready_replicas: ReplicaCount,
    available_replicas: ReplicaCount,
    updated_replicas: ReplicaCount,
}

impl AgentDeploymentStatus {
    /// Creates status after validating replica-count relationships.
    pub fn new(
        observed_version: ResourceVersion,
        desired_replicas: ReplicaCount,
        ready_replicas: ReplicaCount,
        available_replicas: ReplicaCount,
        updated_replicas: ReplicaCount,
    ) -> Result<Self, DeploymentStatusError> {
        if ready_replicas > desired_replicas {
            return Err(DeploymentStatusError::ReadyExceedsDesired);
        }
        if available_replicas > ready_replicas {
            return Err(DeploymentStatusError::AvailableExceedsReady);
        }
        if updated_replicas > desired_replicas {
            return Err(DeploymentStatusError::UpdatedExceedsDesired);
        }
        Ok(Self {
            observed_version,
            desired_replicas,
            ready_replicas,
            available_replicas,
            updated_replicas,
        })
    }

    /// Creates an unreconciled status for a new deployment.
    #[must_use]
    pub fn pending(observed_version: ResourceVersion, desired_replicas: ReplicaCount) -> Self {
        let zero = ReplicaCount::new(0).expect("zero replicas are valid");
        Self {
            observed_version,
            desired_replicas,
            ready_replicas: zero,
            available_replicas: zero,
            updated_replicas: zero,
        }
    }

    /// Returns the observed resource version.
    #[must_use]
    pub const fn observed_version(&self) -> ResourceVersion {
        self.observed_version
    }

    /// Returns replicas expected by the controller.
    #[must_use]
    pub const fn desired_replicas(&self) -> ReplicaCount {
        self.desired_replicas
    }

    /// Returns instances reporting readiness.
    #[must_use]
    pub const fn ready_replicas(&self) -> ReplicaCount {
        self.ready_replicas
    }

    /// Returns ready instances that satisfy availability requirements.
    #[must_use]
    pub const fn available_replicas(&self) -> ReplicaCount {
        self.available_replicas
    }

    /// Returns instances using the current template.
    #[must_use]
    pub const fn updated_replicas(&self) -> ReplicaCount {
        self.updated_replicas
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentDeploymentStatusWire {
    observed_version: ResourceVersion,
    desired_replicas: ReplicaCount,
    ready_replicas: ReplicaCount,
    available_replicas: ReplicaCount,
    updated_replicas: ReplicaCount,
}

impl<'de> Deserialize<'de> for AgentDeploymentStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AgentDeploymentStatusWire::deserialize(deserializer)?;
        Self::new(
            wire.observed_version,
            wire.desired_replicas,
            wire.ready_replicas,
            wire.available_replicas,
            wire.updated_replicas,
        )
        .map_err(de::Error::custom)
    }
}

/// Invalid deployment desired or observed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentStatusError {
    /// The selector does not match its own pod template.
    SelectorDoesNotMatchTemplate,
    /// Ready replicas exceed desired replicas.
    ReadyExceedsDesired,
    /// Available replicas exceed ready replicas.
    AvailableExceedsReady,
    /// Updated replicas exceed desired replicas.
    UpdatedExceedsDesired,
    /// Status and specification disagree about the desired replica count.
    DesiredCountMismatch,
}

impl fmt::Display for DeploymentStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::SelectorDoesNotMatchTemplate => "selector does not match deployment template",
            Self::ReadyExceedsDesired => "ready replicas exceed desired replicas",
            Self::AvailableExceedsReady => "available replicas exceed ready replicas",
            Self::UpdatedExceedsDesired => "updated replicas exceed desired replicas",
            Self::DesiredCountMismatch => {
                "status desired replicas do not match the deployment specification"
            }
        };
        formatter.write_str(message)
    }
}

impl Error for DeploymentStatusError {}

/// Wire document used for an agent deployment.
pub type AgentDeploymentDocument = ResourceDocument<AgentDeploymentSpec, AgentDeploymentStatus>;

/// Invalid resource type or state encountered while restoring a deployment document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentDeploymentDocumentError {
    /// The wire `apiVersion` or `kind` is incorrect.
    Type(AgentDocumentError),
    /// Desired or observed deployment state is inconsistent.
    State(DeploymentStatusError),
}

impl fmt::Display for AgentDeploymentDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type(error) => error.fmt(formatter),
            Self::State(error) => error.fmt(formatter),
        }
    }
}

impl Error for AgentDeploymentDocumentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Type(error) => Some(error),
            Self::State(error) => Some(error),
        }
    }
}

/// Declarative controller resource maintaining a desired number of agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDeployment {
    metadata: Metadata,
    spec: AgentDeploymentSpec,
    status: AgentDeploymentStatus,
}

impl AgentDeployment {
    /// Current AgentDeployment API version.
    pub const API_VERSION: &'static str = "agentkube.ai/v1";
    /// Resource kind used on the wire.
    pub const KIND: &'static str = "AgentDeployment";

    /// Creates a validated deployment with pending observed status.
    pub fn new(
        metadata: Metadata,
        spec: AgentDeploymentSpec,
    ) -> Result<Self, DeploymentStatusError> {
        spec.validate()?;
        let status = AgentDeploymentStatus::pending(metadata.resource_version(), spec.replicas());
        Ok(Self {
            metadata,
            spec,
            status,
        })
    }

    /// Reconstructs a deployment from its wire representation.
    pub fn from_document(
        document: AgentDeploymentDocument,
    ) -> Result<Self, AgentDeploymentDocumentError> {
        let (type_meta, metadata, spec, status) = document.into_parts();
        validate_type_meta(&type_meta, Self::KIND).map_err(AgentDeploymentDocumentError::Type)?;
        spec.validate()
            .map_err(AgentDeploymentDocumentError::State)?;
        let status = status.unwrap_or_else(|| {
            AgentDeploymentStatus::pending(metadata.resource_version(), spec.replicas())
        });
        if status.desired_replicas() != spec.replicas() {
            return Err(AgentDeploymentDocumentError::State(
                DeploymentStatusError::DesiredCountMismatch,
            ));
        }
        Ok(Self {
            metadata,
            spec,
            status,
        })
    }

    /// Converts the deployment into its wire representation.
    #[must_use]
    pub fn into_document(self) -> AgentDeploymentDocument {
        ResourceDocument::new(agent_type_meta(Self::KIND), self.metadata, self.spec)
            .with_status(self.status)
    }

    /// Returns desired deployment state.
    #[must_use]
    pub const fn spec(&self) -> &AgentDeploymentSpec {
        &self.spec
    }

    /// Returns mutable desired deployment state.
    #[must_use]
    pub const fn spec_mut(&mut self) -> &mut AgentDeploymentSpec {
        &mut self.spec
    }

    /// Returns observed deployment state.
    #[must_use]
    pub const fn status(&self) -> &AgentDeploymentStatus {
        &self.status
    }

    /// Replaces status when it describes the current desired replica count.
    pub fn set_status(
        &mut self,
        status: AgentDeploymentStatus,
    ) -> Result<(), DeploymentStatusError> {
        if status.desired_replicas() != self.spec.replicas() {
            return Err(DeploymentStatusError::DesiredCountMismatch);
        }
        self.status = status;
        Ok(())
    }
}

impl Resource for AgentDeployment {
    const API_VERSION: &'static str = Self::API_VERSION;
    const KIND: &'static str = Self::KIND;

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Instructions, ModelName, ModelPolicy, ProviderName};

    fn template() -> AgentSpec {
        AgentSpec::new(
            AgentRole::new("developer").unwrap(),
            ModelPolicy::fixed(
                ProviderName::new("openai").unwrap(),
                ModelName::new("gpt-5").unwrap(),
            ),
            Instructions::new("Implement the assigned task.").unwrap(),
        )
        .with_capability(Capability::new("coding").unwrap())
    }

    #[test]
    fn selector_matches_role_and_all_capabilities() {
        let selector = AgentSelector::any()
            .with_role(AgentRole::new("developer").unwrap())
            .with_capability(Capability::new("coding").unwrap());

        assert!(selector.matches(&template()));
    }

    #[test]
    fn deployment_rejects_a_selector_that_excludes_its_template() {
        let selector = AgentSelector::any().with_role(AgentRole::new("researcher").unwrap());
        let spec = AgentDeploymentSpec::new(ReplicaCount::new(3).unwrap(), template())
            .with_selector(selector);

        assert_eq!(
            AgentDeployment::new(Metadata::new("developers").unwrap(), spec),
            Err(DeploymentStatusError::SelectorDoesNotMatchTemplate)
        );
    }

    #[test]
    fn status_rejects_impossible_replica_counts() {
        let result = AgentDeploymentStatus::new(
            ResourceVersion::INITIAL,
            ReplicaCount::new(2).unwrap(),
            ReplicaCount::new(3).unwrap(),
            ReplicaCount::new(2).unwrap(),
            ReplicaCount::new(2).unwrap(),
        );

        assert_eq!(result, Err(DeploymentStatusError::ReadyExceedsDesired));
    }

    #[test]
    fn deployment_round_trips_through_wire_document() {
        let original = AgentDeployment::new(
            Metadata::new("coding-agents").unwrap(),
            AgentDeploymentSpec::new(ReplicaCount::new(3).unwrap(), template()),
        )
        .unwrap();
        let json = serde_json::to_string(&original.clone().into_document()).unwrap();
        let document: AgentDeploymentDocument = serde_json::from_str(&json).unwrap();
        let decoded = AgentDeployment::from_document(document).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn replica_validation_survives_deserialization() {
        assert!(serde_json::from_str::<ReplicaCount>("10001").is_err());
    }

    #[test]
    fn status_relationships_survive_deserialization() {
        let json = r#"{
            "observedVersion":1,
            "desiredReplicas":2,
            "readyReplicas":3,
            "availableReplicas":1,
            "updatedReplicas":2
        }"#;

        assert!(serde_json::from_str::<AgentDeploymentStatus>(json).is_err());
    }

    #[test]
    fn selector_template_consistency_survives_deserialization() {
        let spec = AgentDeploymentSpec::new(ReplicaCount::new(1).unwrap(), template())
            .with_selector(AgentSelector::any().with_role(AgentRole::new("researcher").unwrap()));
        let json = serde_json::to_string(&spec).unwrap();

        assert!(serde_json::from_str::<AgentDeploymentSpec>(&json).is_err());
    }

    #[test]
    fn deployment_rejects_status_for_a_different_desired_count() {
        let mut deployment = AgentDeployment::new(
            Metadata::new("coding-agents").unwrap(),
            AgentDeploymentSpec::new(ReplicaCount::new(3).unwrap(), template()),
        )
        .unwrap();
        let status =
            AgentDeploymentStatus::pending(ResourceVersion::INITIAL, ReplicaCount::new(2).unwrap());

        assert_eq!(
            deployment.set_status(status),
            Err(DeploymentStatusError::DesiredCountMismatch)
        );
    }
}
