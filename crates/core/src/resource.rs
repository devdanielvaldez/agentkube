use crate::Metadata;

/// Common contract implemented by every declarative AgentKube resource.
pub trait Resource {
    /// API version used when the resource is serialized.
    const API_VERSION: &'static str;

    /// Stable resource kind, such as `Agent` or `AgentTask`.
    const KIND: &'static str;

    /// Returns the resource metadata.
    fn metadata(&self) -> &Metadata;

    /// Returns mutable resource metadata for persistence and reconciliation.
    fn metadata_mut(&mut self) -> &mut Metadata;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceVersion;

    struct TestResource {
        metadata: Metadata,
    }

    impl Resource for TestResource {
        const API_VERSION: &'static str = "agentkube.ai/v1";
        const KIND: &'static str = "TestResource";

        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        fn metadata_mut(&mut self) -> &mut Metadata {
            &mut self.metadata
        }
    }

    #[test]
    fn resource_exposes_its_identity_and_kind() {
        let resource = TestResource {
            metadata: Metadata::new("example").expect("valid metadata"),
        };

        assert_eq!(TestResource::API_VERSION, "agentkube.ai/v1");
        assert_eq!(TestResource::KIND, "TestResource");
        assert_eq!(resource.metadata().name().as_str(), "example");
    }

    #[test]
    fn resource_metadata_can_be_updated_by_reconciliation() {
        let mut resource = TestResource {
            metadata: Metadata::new("example").expect("valid metadata"),
        };

        resource
            .metadata_mut()
            .set_resource_version(ResourceVersion::new(2).unwrap());

        assert_eq!(resource.metadata().resource_version().get(), 2);
    }
}
