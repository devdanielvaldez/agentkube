use agentkube_core::{Metadata, Namespace, Resource, ResourceVersion, TaskId};

struct ExampleResource {
    metadata: Metadata,
}

impl Resource for ExampleResource {
    const API_VERSION: &'static str = "agentkube.ai/v1";
    const KIND: &'static str = "Example";

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }
}

#[test]
fn public_primitives_support_a_resource_lifecycle() {
    let namespace = Namespace::new("engineering").expect("valid namespace");
    let mut resource = ExampleResource {
        metadata: Metadata::in_namespace("backend-agent", namespace).expect("valid metadata"),
    };

    let task_id = TaskId::new();
    let parsed_task_id: TaskId = task_id.to_string().parse().expect("valid task ID");
    resource
        .metadata_mut()
        .set_resource_version(ResourceVersion::INITIAL.next().expect("next version"));

    assert_eq!(parsed_task_id, task_id);
    assert_eq!(ExampleResource::API_VERSION, "agentkube.ai/v1");
    assert_eq!(ExampleResource::KIND, "Example");
    assert_eq!(resource.metadata().name().as_str(), "backend-agent");
    assert_eq!(resource.metadata().namespace().as_str(), "engineering");
    assert_eq!(resource.metadata().resource_version().get(), 2);
}
