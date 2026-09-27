use crate::{AgentSpec, AgentStatus};
use agentkube_core::{Metadata, Resource};
use agentkube_protocol::{ApiVersion, ResourceDocument, ResourceKind, TypeMeta};
use std::{error::Error, fmt};

/// Wire document used to create or retrieve an agent definition.
pub type AgentDocument = ResourceDocument<AgentSpec, AgentStatus>;

/// Desired and observed state of one declarative agent resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    metadata: Metadata,
    spec: AgentSpec,
    status: AgentStatus,
}

impl AgentDefinition {
    /// Current Agent resource API version.
    pub const API_VERSION: &'static str = "agentkube.ai/v1";
    /// Resource kind used on the wire.
    pub const KIND: &'static str = "Agent";

    /// Creates a definition with pending observed state.
    #[must_use]
    pub fn new(metadata: Metadata, spec: AgentSpec) -> Self {
        let status = AgentStatus::pending(metadata.resource_version());
        Self {
            metadata,
            spec,
            status,
        }
    }

    /// Reconstructs a domain resource after validating its wire type metadata.
    pub fn from_document(document: AgentDocument) -> Result<Self, AgentDocumentError> {
        let (type_meta, metadata, spec, status) = document.into_parts();
        validate_type_meta(&type_meta, Self::KIND)?;
        let status = status.unwrap_or_else(|| AgentStatus::pending(metadata.resource_version()));
        Ok(Self {
            metadata,
            spec,
            status,
        })
    }

    /// Converts the definition into its stable wire representation.
    #[must_use]
    pub fn into_document(self) -> AgentDocument {
        ResourceDocument::new(agent_type_meta(Self::KIND), self.metadata, self.spec)
            .with_status(self.status)
    }

    /// Returns the desired agent specification.
    #[must_use]
    pub const fn spec(&self) -> &AgentSpec {
        &self.spec
    }

    /// Returns mutable desired agent state.
    #[must_use]
    pub const fn spec_mut(&mut self) -> &mut AgentSpec {
        &mut self.spec
    }

    /// Returns the controller-maintained status.
    #[must_use]
    pub const fn status(&self) -> &AgentStatus {
        &self.status
    }

    /// Returns mutable controller-maintained status.
    #[must_use]
    pub const fn status_mut(&mut self) -> &mut AgentStatus {
        &mut self.status
    }
}

impl Resource for AgentDefinition {
    const API_VERSION: &'static str = Self::API_VERSION;
    const KIND: &'static str = Self::KIND;

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }
}

/// Error returned when a wire document claims an unexpected resource type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDocumentError {
    expected_api_version: &'static str,
    expected_kind: &'static str,
    actual_api_version: String,
    actual_kind: String,
}

impl AgentDocumentError {
    /// Returns the expected API version.
    #[must_use]
    pub const fn expected_api_version(&self) -> &'static str {
        self.expected_api_version
    }

    /// Returns the expected resource kind.
    #[must_use]
    pub const fn expected_kind(&self) -> &'static str {
        self.expected_kind
    }

    /// Returns the received API version.
    #[must_use]
    pub fn actual_api_version(&self) -> &str {
        &self.actual_api_version
    }

    /// Returns the received resource kind.
    #[must_use]
    pub fn actual_kind(&self) -> &str {
        &self.actual_kind
    }
}

impl fmt::Display for AgentDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "expected {}/{}, received {}/{}",
            self.expected_api_version,
            self.expected_kind,
            self.actual_api_version,
            self.actual_kind
        )
    }
}

impl Error for AgentDocumentError {}

pub(crate) fn agent_type_meta(kind: &str) -> TypeMeta {
    TypeMeta::new(
        ApiVersion::new(AgentDefinition::API_VERSION)
            .expect("AgentKube's static API version is valid"),
        ResourceKind::new(kind).expect("AgentKube's static resource kind is valid"),
    )
}

pub(crate) fn validate_type_meta(
    type_meta: &TypeMeta,
    expected_kind: &'static str,
) -> Result<(), AgentDocumentError> {
    if type_meta.api_version().as_str() == AgentDefinition::API_VERSION
        && type_meta.kind().as_str() == expected_kind
    {
        Ok(())
    } else {
        Err(AgentDocumentError {
            expected_api_version: AgentDefinition::API_VERSION,
            expected_kind,
            actual_api_version: type_meta.api_version().to_string(),
            actual_kind: type_meta.kind().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentRole, Instructions, ModelName, ModelPolicy, ProviderName};
    use agentkube_core::ResourceVersion;

    fn definition() -> AgentDefinition {
        AgentDefinition::new(
            Metadata::new("backend-developer").unwrap(),
            AgentSpec::new(
                AgentRole::new("developer").unwrap(),
                ModelPolicy::fixed(
                    ProviderName::new("openai").unwrap(),
                    ModelName::new("gpt-5").unwrap(),
                ),
                Instructions::new("Build the backend.").unwrap(),
            ),
        )
    }

    #[test]
    fn definition_round_trips_through_a_wire_document() {
        let original = definition();
        let encoded = serde_json::to_string(&original.clone().into_document()).unwrap();
        let document: AgentDocument = serde_json::from_str(&encoded).unwrap();
        let decoded = AgentDefinition::from_document(document).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn rejects_a_document_with_the_wrong_kind() {
        let original = definition();
        let (_, metadata, spec, status) = original.into_document().into_parts();
        let document = ResourceDocument::new(agent_type_meta("AgentTask"), metadata, spec)
            .with_status(status.unwrap());

        assert!(AgentDefinition::from_document(document).is_err());
    }

    #[test]
    fn missing_wire_status_defaults_to_pending() {
        let original = definition();
        let (type_meta, metadata, spec, _) = original.into_document().into_parts();
        let document: AgentDocument = ResourceDocument::without_status(type_meta, metadata, spec);
        let decoded = AgentDefinition::from_document(document).unwrap();

        assert_eq!(decoded.status().phase(), crate::AgentPhase::Pending);
        assert_eq!(
            decoded.status().observed_version(),
            ResourceVersion::INITIAL
        );
    }
}
