use crate::{Namespace, ResourceName, ResourceUid, ResourceVersion, ValidationError};
use serde::{Deserialize, Serialize};

/// Identity and concurrency metadata shared by every AgentKube resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    uid: ResourceUid,
    name: ResourceName,
    namespace: Namespace,
    resource_version: ResourceVersion,
}

impl Metadata {
    /// Creates metadata in the `default` namespace.
    pub fn new(name: impl Into<String>) -> Result<Self, ValidationError> {
        Self::in_namespace(name, Namespace::default())
    }

    /// Creates metadata in a specific namespace.
    pub fn in_namespace(
        name: impl Into<String>,
        namespace: Namespace,
    ) -> Result<Self, ValidationError> {
        Ok(Self {
            uid: ResourceUid::new(),
            name: ResourceName::new(name)?,
            namespace,
            resource_version: ResourceVersion::INITIAL,
        })
    }

    /// Returns the stable UID of the resource.
    #[must_use]
    pub const fn uid(&self) -> ResourceUid {
        self.uid
    }

    /// Returns the resource name.
    #[must_use]
    pub const fn name(&self) -> &ResourceName {
        &self.name
    }

    /// Returns the namespace containing the resource.
    #[must_use]
    pub const fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// Returns the current resource version.
    #[must_use]
    pub const fn resource_version(&self) -> ResourceVersion {
        self.resource_version
    }

    /// Replaces the resource version after a successful persisted update.
    pub fn set_resource_version(&mut self, version: ResourceVersion) {
        self.resource_version = version;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_metadata_has_expected_defaults() {
        let metadata = Metadata::new("backend-agent").expect("valid metadata");

        assert_eq!(metadata.name().as_str(), "backend-agent");
        assert_eq!(metadata.namespace().as_str(), "default");
        assert_eq!(metadata.resource_version(), ResourceVersion::INITIAL);
    }

    #[test]
    fn metadata_round_trips_through_json() {
        let original = Metadata::in_namespace(
            "backend-agent",
            Namespace::new("production").expect("valid namespace"),
        )
        .expect("valid metadata");

        let json = serde_json::to_string(&original).expect("serialize metadata");
        let decoded: Metadata = serde_json::from_str(&json).expect("deserialize metadata");

        assert_eq!(decoded, original);
    }

    #[test]
    fn invalid_name_is_rejected_at_construction() {
        assert!(Metadata::new("Invalid Name").is_err());
    }
}
