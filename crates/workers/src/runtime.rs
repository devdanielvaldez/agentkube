use crate::{ExecutionClaim, ToolRegistry, ToolRegistryError};
use agentkube_agents::{ModelName, ModelPolicy};
use agentkube_providers::{
    ChatMessage, FinishReason, GenerationRequest, MessageText, ProviderErrorKind,
};
use agentkube_router::{
    ModelRouting, RouteDecision, RouterError, RoutingConstraints, RoutingRequest,
};
use agentkube_tasks::{TaskFailureKind, TaskResult, TaskUsage};
use std::{
    error::Error,
    fmt,
    future::Future,
    num::{NonZeroU16, NonZeroU32},
    pin::Pin,
    sync::Arc,
};

/// Sendable future returned by agent runtime operations.
pub type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, RuntimeError>> + Send + 'a>>;

/// Runtime liveness reported to a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeHealth {
    /// New executions may start.
    Healthy,
    /// Existing work may finish, but new work should be avoided.
    Degraded,
    /// The runtime cannot execute work.
    Unavailable,
}

/// Successful runtime output and the model route that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutput {
    result: TaskResult,
    route: RouteDecision,
}

impl RuntimeOutput {
    /// Creates runtime output from a task result and route decision.
    #[must_use]
    pub const fn new(result: TaskResult, route: RouteDecision) -> Self {
        Self { result, route }
    }

    /// Returns the task result to persist.
    #[must_use]
    pub const fn result(&self) -> &TaskResult {
        &self.result
    }

    /// Returns the successful model route.
    #[must_use]
    pub const fn route(&self) -> &RouteDecision {
        &self.route
    }
}

/// Runtime-independent execution contract used by workers.
pub trait AgentRuntime: Send + Sync {
    /// Reports whether new execution may start.
    fn health<'a>(&'a self) -> RuntimeFuture<'a, RuntimeHealth>;

    /// Executes one atomically claimed task.
    fn execute<'a>(&'a self, claim: ExecutionClaim) -> RuntimeFuture<'a, RuntimeOutput>;
}

/// Single-model-turn runtime backed by AgentKube's model router.
///
/// Tool-enabled agents require a richer runtime adapter and are rejected by
/// this implementation rather than silently ignoring their tool contract.
pub struct SingleTurnRuntime {
    router: Arc<dyn ModelRouting>,
}

impl SingleTurnRuntime {
    /// Creates a runtime over a shared model router.
    #[must_use]
    pub const fn new(router: Arc<dyn ModelRouting>) -> Self {
        Self { router }
    }
}

impl AgentRuntime for SingleTurnRuntime {
    fn health<'a>(&'a self) -> RuntimeFuture<'a, RuntimeHealth> {
        Box::pin(async { Ok(RuntimeHealth::Healthy) })
    }

    fn execute<'a>(&'a self, claim: ExecutionClaim) -> RuntimeFuture<'a, RuntimeOutput> {
        Box::pin(async move {
            if !claim.definition().spec().tools().is_empty() {
                return Err(RuntimeError::UnsupportedTools);
            }
            let policy = claim.definition().spec().model().clone();
            let seed_model = match &policy {
                ModelPolicy::Fixed { model, .. } => model.clone(),
                ModelPolicy::Auto { .. } => {
                    ModelName::new("auto").expect("static model name is valid")
                }
            };
            let messages = vec![
                ChatMessage::system(
                    MessageText::new(claim.definition().spec().instructions().as_str())
                        .map_err(|_| RuntimeError::InvalidPrompt)?,
                ),
                ChatMessage::user(
                    MessageText::new(claim.task().spec().objective().as_str())
                        .map_err(|_| RuntimeError::InvalidPrompt)?,
                ),
            ];
            let mut generation = GenerationRequest::new(seed_model, messages)
                .map_err(|_| RuntimeError::InvalidPrompt)?;
            let max_tokens = [
                claim.task().spec().budget().max_tokens(),
                claim.definition().spec().resources().max_tokens(),
            ]
            .into_iter()
            .flatten()
            .min();
            if let Some(max_tokens) = max_tokens {
                let value = u32::try_from(max_tokens.get())
                    .ok()
                    .and_then(NonZeroU32::new)
                    .ok_or(RuntimeError::OutputLimitTooLarge(max_tokens.get()))?;
                generation = generation.with_max_output_tokens(value);
            }
            let mut constraints = RoutingConstraints::new()
                .with_privacy(claim.task().spec().requirements().privacy());
            let maximum_cost = [
                claim.task().spec().budget().max_cost_micro_usd(),
                claim.definition().spec().resources().max_cost_micro_usd(),
            ]
            .into_iter()
            .flatten()
            .min();
            if let Some(maximum) = maximum_cost {
                constraints = constraints.with_maximum_cost(maximum.get());
            }
            let routed = self
                .router
                .generate(RoutingRequest::new(policy, generation).with_constraints(constraints))
                .await
                .map_err(RuntimeError::Routing)?;
            if routed.response().finish_reason() == FinishReason::ContentFilter {
                return Err(RuntimeError::ContentFiltered);
            }
            if !routed.response().generated_tool_calls().is_empty() {
                return Err(RuntimeError::UnexpectedToolCalls);
            }
            let output = routed
                .response()
                .generated_text()
                .ok_or(RuntimeError::MissingOutput)?
                .to_owned();
            let usage = routed.response().usage();
            let cost = routed
                .decision()
                .list_cost_for_usage(usage)
                .ok_or(RuntimeError::CostOverflow)?;
            let result = TaskResult::new(
                output,
                TaskUsage::new(usage.input_tokens(), usage.output_tokens(), cost),
            );
            Ok(RuntimeOutput::new(result, routed.decision().clone()))
        })
    }
}

/// Bounded multi-turn runtime with explicit, allowlisted tool execution.
///
/// Every tool must be both declared by the agent and registered in the local
/// [`ToolRegistry`]. Token and cost usage accumulate across all turns.
pub struct AgenticRuntime {
    router: Arc<dyn ModelRouting>,
    tools: ToolRegistry,
    max_turns: NonZeroU16,
}

impl AgenticRuntime {
    /// Creates a bounded agent loop.
    #[must_use]
    pub const fn new(
        router: Arc<dyn ModelRouting>,
        tools: ToolRegistry,
        max_turns: NonZeroU16,
    ) -> Self {
        Self {
            router,
            tools,
            max_turns,
        }
    }
}

impl AgentRuntime for AgenticRuntime {
    fn health<'a>(&'a self) -> RuntimeFuture<'a, RuntimeHealth> {
        Box::pin(async { Ok(RuntimeHealth::Healthy) })
    }

    fn execute<'a>(&'a self, claim: ExecutionClaim) -> RuntimeFuture<'a, RuntimeOutput> {
        Box::pin(async move {
            let policy = claim.definition().spec().model().clone();
            let seed_model = match &policy {
                ModelPolicy::Fixed { model, .. } => model.clone(),
                ModelPolicy::Auto { .. } => {
                    ModelName::new("auto").expect("static model name is valid")
                }
            };
            let definitions = self
                .tools
                .definitions(claim.definition().spec().tools())
                .map_err(RuntimeError::Tools)?;
            let allowed_tools = claim.definition().spec().tools();
            let mut messages = vec![
                ChatMessage::system(
                    MessageText::new(claim.definition().spec().instructions().as_str())
                        .map_err(|_| RuntimeError::InvalidPrompt)?,
                ),
                ChatMessage::user(
                    MessageText::new(claim.task().spec().objective().as_str())
                        .map_err(|_| RuntimeError::InvalidPrompt)?,
                ),
            ];
            let token_limit = [
                claim.task().spec().budget().max_tokens(),
                claim.definition().spec().resources().max_tokens(),
            ]
            .into_iter()
            .flatten()
            .min()
            .map(|value| value.get());
            let cost_limit = [
                claim.task().spec().budget().max_cost_micro_usd(),
                claim.definition().spec().resources().max_cost_micro_usd(),
            ]
            .into_iter()
            .flatten()
            .min()
            .map(|value| value.get());
            let mut input_tokens = 0u64;
            let mut output_tokens = 0u64;
            let mut total_cost = 0u64;

            for _ in 0..self.max_turns.get() {
                let mut generation = GenerationRequest::new(seed_model.clone(), messages.clone())
                    .map_err(|_| RuntimeError::InvalidPrompt)?
                    .with_tools(definitions.clone())
                    .map_err(|_| RuntimeError::InvalidPrompt)?;
                if let Some(limit) = token_limit {
                    let consumed = input_tokens
                        .checked_add(output_tokens)
                        .ok_or(RuntimeError::CostOverflow)?;
                    let remaining = limit
                        .checked_sub(consumed)
                        .and_then(|value| u32::try_from(value).ok())
                        .and_then(NonZeroU32::new)
                        .ok_or(RuntimeError::TokenBudgetExceeded)?;
                    generation = generation.with_max_output_tokens(remaining);
                }
                let mut constraints = RoutingConstraints::new()
                    .with_privacy(claim.task().spec().requirements().privacy());
                if let Some(limit) = cost_limit {
                    let remaining = limit
                        .checked_sub(total_cost)
                        .ok_or(RuntimeError::CostBudgetExceeded)?;
                    constraints = constraints.with_maximum_cost(remaining);
                }
                let routed = self
                    .router
                    .generate(
                        RoutingRequest::new(policy.clone(), generation)
                            .with_constraints(constraints),
                    )
                    .await
                    .map_err(RuntimeError::Routing)?;
                let usage = routed.response().usage();
                input_tokens = input_tokens
                    .checked_add(usage.input_tokens())
                    .ok_or(RuntimeError::CostOverflow)?;
                output_tokens = output_tokens
                    .checked_add(usage.output_tokens())
                    .ok_or(RuntimeError::CostOverflow)?;
                total_cost = total_cost
                    .checked_add(
                        routed
                            .decision()
                            .list_cost_for_usage(usage)
                            .ok_or(RuntimeError::CostOverflow)?,
                    )
                    .ok_or(RuntimeError::CostOverflow)?;
                if token_limit.is_some_and(|limit| {
                    input_tokens
                        .checked_add(output_tokens)
                        .is_none_or(|total| total > limit)
                }) {
                    return Err(RuntimeError::TokenBudgetExceeded);
                }
                if cost_limit.is_some_and(|limit| total_cost > limit) {
                    return Err(RuntimeError::CostBudgetExceeded);
                }
                if routed.response().finish_reason() == FinishReason::ContentFilter {
                    return Err(RuntimeError::ContentFiltered);
                }
                let calls = routed.response().generated_tool_calls().to_vec();
                if calls.is_empty() {
                    let output = routed
                        .response()
                        .generated_text()
                        .ok_or(RuntimeError::MissingOutput)?
                        .to_owned();
                    return Ok(RuntimeOutput::new(
                        TaskResult::new(
                            output,
                            TaskUsage::new(input_tokens, output_tokens, total_cost),
                        ),
                        routed.decision().clone(),
                    ));
                }
                if calls
                    .iter()
                    .any(|call| !allowed_tools.contains(call.name()))
                {
                    return Err(RuntimeError::UnauthorizedToolCall);
                }
                let assistant_text = routed
                    .response()
                    .generated_text()
                    .map(MessageText::new)
                    .transpose()
                    .map_err(|_| RuntimeError::InvalidPrompt)?;
                messages.push(
                    ChatMessage::assistant_tool_calls(assistant_text, calls.clone())
                        .map_err(|_| RuntimeError::InvalidPrompt)?,
                );
                for call in calls {
                    let output = self
                        .tools
                        .execute(call.name(), call.arguments())
                        .await
                        .map_err(RuntimeError::Tools)?;
                    messages.push(ChatMessage::tool(
                        call.id().clone(),
                        MessageText::new(output).map_err(|_| RuntimeError::InvalidPrompt)?,
                    ));
                }
            }
            Err(RuntimeError::TurnLimitExceeded(self.max_turns.get()))
        })
    }
}

/// Failure returned by an agent runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// The runtime cannot yet execute agents requiring tools.
    UnsupportedTools,
    /// Instructions or objective could not form a portable prompt.
    InvalidPrompt,
    /// The requested task output limit exceeds the provider protocol range.
    OutputLimitTooLarge(u64),
    /// Model routing or provider execution failed.
    Routing(RouterError),
    /// Provider safety systems filtered the output.
    ContentFiltered,
    /// A single-turn runtime unexpectedly received tool calls.
    UnexpectedToolCalls,
    /// The provider returned neither text nor a supported output type.
    MissingOutput,
    /// Estimated cost could not be represented.
    CostOverflow,
    /// Accumulated input and output tokens crossed the declared budget.
    TokenBudgetExceeded,
    /// Accumulated provider cost crossed the declared budget.
    CostBudgetExceeded,
    /// A model requested a tool not granted to the agent.
    UnauthorizedToolCall,
    /// Tool lookup or execution failed.
    Tools(ToolRegistryError),
    /// The agent did not finish within its bounded turn count.
    TurnLimitExceeded(u16),
    /// Runtime health does not allow new work.
    Unavailable,
}

impl RuntimeError {
    /// Maps the runtime failure into stable task retry semantics.
    #[must_use]
    pub fn task_failure_kind(&self) -> TaskFailureKind {
        match self {
            Self::Routing(RouterError::ProviderRejected { error, .. }) => {
                provider_failure_kind(error.kind())
            }
            Self::Routing(RouterError::AllProvidersFailed(errors)) => errors
                .last()
                .map_or(TaskFailureKind::ProviderError, |error| {
                    provider_failure_kind(error.kind())
                }),
            Self::Routing(_) | Self::Unavailable => TaskFailureKind::ProviderError,
            Self::ContentFiltered
            | Self::UnsupportedTools
            | Self::InvalidPrompt
            | Self::OutputLimitTooLarge(_)
            | Self::UnexpectedToolCalls
            | Self::MissingOutput
            | Self::UnauthorizedToolCall
            | Self::Tools(_) => TaskFailureKind::Validation,
            Self::CostOverflow
            | Self::TokenBudgetExceeded
            | Self::CostBudgetExceeded
            | Self::TurnLimitExceeded(_) => TaskFailureKind::BudgetExceeded,
        }
    }
}

fn provider_failure_kind(kind: ProviderErrorKind) -> TaskFailureKind {
    match kind {
        ProviderErrorKind::RateLimited { .. } => TaskFailureKind::RateLimit,
        ProviderErrorKind::Timeout => TaskFailureKind::Timeout,
        ProviderErrorKind::InvalidRequest
        | ProviderErrorKind::ModelNotFound
        | ProviderErrorKind::ContextLengthExceeded
        | ProviderErrorKind::ContentFiltered
        | ProviderErrorKind::Protocol => TaskFailureKind::Validation,
        ProviderErrorKind::Authentication => TaskFailureKind::PermissionDenied,
        ProviderErrorKind::Unavailable | ProviderErrorKind::Internal => {
            TaskFailureKind::ProviderError
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTools => {
                formatter.write_str("single-turn runtime does not support tools")
            }
            Self::InvalidPrompt => formatter.write_str("runtime prompt is invalid"),
            Self::OutputLimitTooLarge(value) => {
                write!(formatter, "output limit {value} exceeds u32")
            }
            Self::Routing(error) => write!(formatter, "model routing failed: {error}"),
            Self::ContentFiltered => formatter.write_str("provider filtered the generated content"),
            Self::UnexpectedToolCalls => {
                formatter.write_str("runtime received unexpected tool calls")
            }
            Self::MissingOutput => formatter.write_str("provider returned no generated text"),
            Self::CostOverflow => formatter.write_str("runtime cost exceeded supported range"),
            Self::TokenBudgetExceeded => formatter.write_str("token budget exceeded"),
            Self::CostBudgetExceeded => formatter.write_str("cost budget exceeded"),
            Self::UnauthorizedToolCall => {
                formatter.write_str("model requested an unauthorized tool")
            }
            Self::Tools(error) => write!(formatter, "tool execution failed: {error}"),
            Self::TurnLimitExceeded(limit) => write!(formatter, "agent exceeded {limit} turns"),
            Self::Unavailable => formatter.write_str("agent runtime is unavailable"),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Routing(error) => Some(error),
            Self::Tools(error) => Some(error),
            _ => None,
        }
    }
}
