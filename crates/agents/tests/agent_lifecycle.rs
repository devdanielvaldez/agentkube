use agentkube_agents::{
    AgentDefinition, AgentInstance, AgentInstanceState, AgentPermissions, AgentRole, AgentSpec,
    Capability, FilesystemAccess, Instructions, ModelName, ModelPolicy, NetworkAccess,
    ProviderName, ToolName,
};
use agentkube_core::{Metadata, NodeId, Resource, TaskId};

#[test]
fn public_api_supports_definition_scheduling_and_task_execution() {
    let spec = AgentSpec::new(
        AgentRole::new("software-engineer").expect("valid role"),
        ModelPolicy::fixed(
            ProviderName::new("openai").expect("valid provider"),
            ModelName::new("gpt-5").expect("valid model"),
        ),
        Instructions::new("Implement and test the assigned backend task.")
            .expect("valid instructions"),
    )
    .with_capability(Capability::new("coding").expect("valid capability"))
    .with_tool(ToolName::new("github").expect("valid tool"))
    .with_permissions(
        AgentPermissions::denied()
            .with_network(NetworkAccess::Restricted)
            .with_filesystem(FilesystemAccess::Workspace)
            .with_shell(true),
    );
    let definition = AgentDefinition::new(
        Metadata::new("backend-developer").expect("valid metadata"),
        spec,
    );
    let document = definition.clone().into_document();
    let restored = AgentDefinition::from_document(document).expect("valid document");

    let mut instance = AgentInstance::new(restored.metadata().uid());
    instance
        .transition(AgentInstanceState::Scheduling)
        .expect("schedule instance");
    instance.assign_node(NodeId::new()).expect("assign node");
    instance
        .transition(AgentInstanceState::Starting)
        .expect("start runtime");
    instance
        .transition(AgentInstanceState::Ready)
        .expect("runtime ready");

    let task = TaskId::new();
    instance.begin_task(task).expect("begin task");
    instance.complete_task(task).expect("complete task");

    assert_eq!(restored, definition);
    assert_eq!(instance.state(), AgentInstanceState::Ready);
    assert_eq!(instance.current_task(), None);
    assert!(instance.revision() > 1);
}
