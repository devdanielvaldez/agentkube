use agentkube_core::Metadata;
use agentkube_protocol::{ApiVersion, ResourceDocument, ResourceKind, TypeMeta, WatchEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSpec {
    role: String,
    replicas: u16,
}

#[test]
fn declarative_resource_round_trips_through_the_public_api() {
    let document = ResourceDocument::new(
        TypeMeta::new(
            ApiVersion::new("agentkube.ai/v1").expect("valid API version"),
            ResourceKind::new("Agent").expect("valid resource kind"),
        ),
        Metadata::new("backend-developer").expect("valid metadata"),
        AgentSpec {
            role: "software-engineer".to_owned(),
            replicas: 3,
        },
    );

    let encoded = serde_json::to_string(&document).expect("serialize resource document");
    let decoded: ResourceDocument<AgentSpec> =
        serde_json::from_str(&encoded).expect("deserialize resource document");

    assert_eq!(decoded, document);
    assert_eq!(decoded.type_meta().api_version().group(), "agentkube.ai");
    assert_eq!(decoded.metadata().name().as_str(), "backend-developer");
}

#[test]
fn resource_documents_can_be_streamed_as_watch_events() {
    let document = ResourceDocument::new(
        TypeMeta::new(
            ApiVersion::new("agentkube.ai/v1").unwrap(),
            ResourceKind::new("Agent").unwrap(),
        ),
        Metadata::new("reviewer").unwrap(),
        AgentSpec {
            role: "reviewer".to_owned(),
            replicas: 1,
        },
    );
    let event = WatchEvent::Added(document);
    let encoded = serde_json::to_value(event).expect("serialize watch event");

    assert_eq!(encoded["type"], "ADDED");
    assert_eq!(encoded["object"]["kind"], "Agent");
    assert_eq!(encoded["object"]["spec"]["replicas"], 1);
}
