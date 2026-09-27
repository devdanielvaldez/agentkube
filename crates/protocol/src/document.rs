use crate::{ApiVersion, ResourceKind};
use agentkube_core::Metadata;
use serde::{Deserialize, Serialize};

/// Identifies the schema used to encode a resource document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeMeta {
    api_version: ApiVersion,
    kind: ResourceKind,
}

impl TypeMeta {
    /// Creates type metadata from validated protocol values.
    #[must_use]
    pub const fn new(api_version: ApiVersion, kind: ResourceKind) -> Self {
        Self { api_version, kind }
    }

    /// Returns the resource API version.
    #[must_use]
    pub const fn api_version(&self) -> &ApiVersion {
        &self.api_version
    }

    /// Returns the resource kind.
    #[must_use]
    pub const fn kind(&self) -> &ResourceKind {
        &self.kind
    }
}

/// Declarative wire representation of an AgentKube resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDocument<Spec, Status = ()> {
    #[serde(flatten)]
    type_meta: TypeMeta,
    metadata: Metadata,
    spec: Spec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<Status>,
}

impl<Spec> ResourceDocument<Spec, ()> {
    /// Creates a resource document without an observed status.
    #[must_use]
    pub const fn new(type_meta: TypeMeta, metadata: Metadata, spec: Spec) -> Self {
        Self {
            type_meta,
            metadata,
            spec,
            status: None,
        }
    }
}

impl<Spec, Status> ResourceDocument<Spec, Status> {
    /// Returns the document type metadata.
    #[must_use]
    pub const fn type_meta(&self) -> &TypeMeta {
        &self.type_meta
    }

    /// Returns the common resource metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns mutable common resource metadata.
    #[must_use]
    pub const fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }

    /// Returns the desired specification.
    #[must_use]
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Returns mutable access to the desired specification.
    #[must_use]
    pub const fn spec_mut(&mut self) -> &mut Spec {
        &mut self.spec
    }

    /// Returns the observed status when one is present.
    #[must_use]
    pub const fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// Attaches an observed status, changing its static type when necessary.
    #[must_use]
    pub fn with_status<NewStatus>(self, status: NewStatus) -> ResourceDocument<Spec, NewStatus> {
        ResourceDocument {
            type_meta: self.type_meta,
            metadata: self.metadata,
            spec: self.spec,
            status: Some(status),
        }
    }

    /// Transforms the specification while preserving metadata and status.
    #[must_use]
    pub fn map_spec<NewSpec>(
        self,
        map: impl FnOnce(Spec) -> NewSpec,
    ) -> ResourceDocument<NewSpec, Status> {
        ResourceDocument {
            type_meta: self.type_meta,
            metadata: self.metadata,
            spec: map(self.spec),
            status: self.status,
        }
    }

    /// Decomposes the document into its protocol parts.
    #[must_use]
    pub fn into_parts(self) -> (TypeMeta, Metadata, Spec, Option<Status>) {
        (self.type_meta, self.metadata, self.spec, self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TestSpec {
        replicas: u16,
    }

    fn document() -> ResourceDocument<TestSpec> {
        ResourceDocument::new(
            TypeMeta::new(
                ApiVersion::new("agentkube.ai/v1").unwrap(),
                ResourceKind::new("AgentDeployment").unwrap(),
            ),
            Metadata::new("coding-agents").unwrap(),
            TestSpec { replicas: 3 },
        )
    }

    #[test]
    fn serializes_to_the_declarative_resource_shape() {
        let value = serde_json::to_value(document()).unwrap();

        assert_eq!(value["apiVersion"], "agentkube.ai/v1");
        assert_eq!(value["kind"], "AgentDeployment");
        assert_eq!(value["metadata"]["name"], "coding-agents");
        assert_eq!(value["spec"]["replicas"], 3);
        assert!(value.get("status").is_none());
    }

    #[test]
    fn status_is_explicitly_attached() {
        let value = serde_json::to_value(document().with_status("Ready")).unwrap();

        assert_eq!(value["status"], "Ready");
    }
}
