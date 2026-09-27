use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use uuid::Uuid;

macro_rules! define_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a new random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Creates a typed identifier from a UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// Consumes the identifier and returns its UUID.
            #[must_use]
            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_uuid(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.into_uuid()
            }
        }
    };
}

define_id!(AgentId, "Unique identifier for an agent instance.");
define_id!(TaskId, "Unique identifier for a task.");
define_id!(NodeId, "Unique identifier for an AgentKube node.");
define_id!(
    WorkflowId,
    "Unique identifier for a workflow or workflow run."
);
define_id!(ProviderId, "Unique identifier for a model provider.");
define_id!(ModelId, "Unique identifier for a model.");
define_id!(
    TraceId,
    "Unique identifier used to correlate a distributed trace."
);
define_id!(
    ResourceUid,
    "Stable unique identifier for an AgentKube resource."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identifiers_are_unique() {
        assert_ne!(TaskId::new(), TaskId::new());
    }

    #[test]
    fn identifier_round_trips_through_its_string_form() {
        let original = AgentId::new();
        let parsed: AgentId = original.to_string().parse().expect("valid agent ID");

        assert_eq!(parsed, original);
    }

    #[test]
    fn identifier_round_trips_through_json() {
        let original = NodeId::new();
        let json = serde_json::to_string(&original).expect("serialize node ID");
        let decoded: NodeId = serde_json::from_str(&json).expect("deserialize node ID");

        assert_eq!(decoded, original);
    }

    #[test]
    fn invalid_identifier_is_rejected() {
        assert!("not-a-uuid".parse::<TaskId>().is_err());
    }
}
