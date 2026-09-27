use agentkube_agents::{Capability, ProviderName, ToolName};
use agentkube_core::{AgentId, Metadata, Resource};
use agentkube_tasks::{
    AgentTask, Objective, PrivacyRequirement, TaskRequirements, TaskResult, TaskSpec, TaskState,
    TaskUsage,
};

#[test]
fn public_api_supports_a_complete_task_lifecycle() {
    let requirements = TaskRequirements::new()
        .with_capability(Capability::new("coding").expect("valid capability"))
        .with_tool(ToolName::new("github").expect("valid tool"))
        .with_privacy(PrivacyRequirement::Confidential)
        .allow_provider(ProviderName::new("openai").expect("valid provider"))
        .expect("consistent provider policy");
    let mut task = AgentTask::new(
        Metadata::new("fix-login-bug").expect("valid metadata"),
        TaskSpec::new(Objective::new("Investigate and fix issue #381.").expect("valid objective"))
            .with_requirements(requirements),
    );
    let task_id = task.status().task_id();

    task.enqueue().expect("enqueue");
    task.schedule(AgentId::new()).expect("schedule");
    task.start().expect("start");
    task.wait_for_approval().expect("wait for approval");
    task.resume().expect("resume");
    task.complete(TaskResult::new(
        "Pull request created",
        TaskUsage::new(1_000, 500, 125_000),
    ))
    .expect("complete");

    let document = task.clone().into_document();
    let restored = AgentTask::from_document(document).expect("restore document");

    assert_eq!(restored, task);
    assert_eq!(restored.status().task_id(), task_id);
    assert_eq!(restored.status().state(), TaskState::Completed);
    assert_eq!(restored.metadata().name().as_str(), "fix-login-bug");
}
