use agentkube_agents::{
    AgentDefinition, AgentInstance, AgentInstanceState, AgentRole, AgentSpec, Instructions,
    ModelName, ModelPolicy, ProviderName, ToolName,
};
use agentkube_core::{Metadata, NodeId, Resource};
use agentkube_providers::{
    FinishReason, GenerationResponse, GenerationUsage, MessageText, ModelCapabilities,
    ModelProvider, ScriptedProvider, ToolCall, ToolCallId, ToolDefinition,
};
use agentkube_router::{
    DataResidency, ModelRouter, ModelRouting, ModelRoutingProfile, ProviderRegistry,
};
use agentkube_tasks::{AgentTask, Objective, TaskSpec};
use agentkube_workers::{
    AgentRuntime, AgenticRuntime, InMemoryWorkerStateStore, ToolExecutionError, ToolExecutor,
    ToolFuture, ToolRegistry, WorkerStateStore,
};
use serde_json::json;
use std::{
    num::{NonZeroU16, NonZeroU32},
    sync::Arc,
};

struct EchoTool {
    definition: ToolDefinition,
}

impl ToolExecutor for EchoTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'a>(&'a self, arguments: &'a serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            arguments
                .get("value")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| ToolExecutionError::new("value must be a string"))
        })
    }
}

#[tokio::test]
async fn runtime_executes_allowlisted_tool_and_accumulates_usage() {
    let provider_name = ProviderName::new("scripted").unwrap();
    let model = ModelName::new("tool-model").unwrap();
    let tool_name = ToolName::new("echo").unwrap();
    let provider = Arc::new(ScriptedProvider::new(
        provider_name.clone(),
        agentkube_providers::ProviderCapabilities::new(vec![(
            model.clone(),
            ModelCapabilities::new(
                NonZeroU32::new(8_192).unwrap(),
                NonZeroU32::new(1_024).unwrap(),
                true,
                false,
                1_000_000,
                2_000_000,
            ),
        )])
        .unwrap(),
    ));
    provider
        .push_response(
            GenerationResponse::tool_calls(
                model.clone(),
                None,
                vec![
                    ToolCall::new(
                        ToolCallId::new("call-1").unwrap(),
                        tool_name.clone(),
                        json!({"value": "observed"}),
                    )
                    .unwrap(),
                ],
                GenerationUsage::new(10, 2, 0),
            )
            .unwrap(),
        )
        .unwrap();
    provider
        .push_response(
            GenerationResponse::text(
                model.clone(),
                MessageText::new("finished").unwrap(),
                FinishReason::Stop,
                GenerationUsage::new(12, 3, 0),
            )
            .unwrap(),
        )
        .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(
            provider as Arc<dyn ModelProvider>,
            vec![(
                model.clone(),
                ModelRoutingProfile::new(8_000, "1s".parse().unwrap(), DataResidency::Local)
                    .unwrap(),
            )],
        )
        .unwrap();
    let runtime = AgenticRuntime::new(
        Arc::new(ModelRouter::new(registry)) as Arc<dyn ModelRouting>,
        ToolRegistry::new(vec![Arc::new(EchoTool {
            definition: ToolDefinition::new(
                tool_name.clone(),
                "Echo a value.",
                json!({"type": "object", "properties": {"value": {"type": "string"}}}),
            )
            .unwrap(),
        })])
        .unwrap(),
        NonZeroU16::new(4).unwrap(),
    );
    let definition = AgentDefinition::new(
        Metadata::new("tool-agent").unwrap(),
        AgentSpec::new(
            AgentRole::new("worker").unwrap(),
            ModelPolicy::fixed(provider_name, model),
            Instructions::new("Use the tool and finish.").unwrap(),
        )
        .with_tool(tool_name),
    );
    let mut instance = AgentInstance::new(definition.metadata().uid());
    let node = NodeId::new();
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(node).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
    let agent_id = instance.id();
    let mut task = AgentTask::new(
        Metadata::new("tool-task").unwrap(),
        TaskSpec::new(Objective::new("Echo this.").unwrap()),
    );
    task.enqueue().unwrap();
    let task_id = task.status().task_id();
    let state = InMemoryWorkerStateStore::new();
    state.insert_definition(definition).unwrap();
    state.insert_agent(instance).unwrap();
    state.insert_task(task).unwrap();
    let claim = state.claim(task_id, agent_id, node).await.unwrap();

    let output = runtime.execute(claim).await.unwrap();
    assert_eq!(output.result().output(), "finished");
    assert_eq!(output.result().usage().input_tokens(), 22);
    assert_eq!(output.result().usage().output_tokens(), 5);
    assert_eq!(output.result().usage().cost_micro_usd(), 32);
}
