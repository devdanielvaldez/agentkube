use agentkube_core::{Metadata, Namespace, ResourceName};
use std::fmt;

/// Stable lookup key for a namespaced declarative resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceKey {
    namespace: Namespace,
    name: ResourceName,
}

impl ResourceKey {
    /// Creates a key from a validated namespace and resource name.
    #[must_use]
    pub const fn new(namespace: Namespace, name: ResourceName) -> Self {
        Self { namespace, name }
    }

    /// Returns the namespace component.
    #[must_use]
    pub const fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// Returns the resource-name component.
    #[must_use]
    pub const fn name(&self) -> &ResourceName {
        &self.name
    }
}

impl From<&Metadata> for ResourceKey {
    fn from(metadata: &Metadata) -> Self {
        Self::new(metadata.namespace().clone(), metadata.name().clone())
    }
}

impl fmt::Display for ResourceKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.namespace, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_derived_from_metadata() {
        let metadata = Metadata::new("planner").unwrap();
        let key = ResourceKey::from(&metadata);

        assert_eq!(key.namespace().as_str(), "default");
        assert_eq!(key.name().as_str(), "planner");
        assert_eq!(key.to_string(), "default/planner");
    }
}
