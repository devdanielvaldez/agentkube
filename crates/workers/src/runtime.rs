use crate::ExecutionClaim;
use agentkube_agents::{ModelName, ModelPolicy};
use agentkube_providers::{
    ChatMessage, FinishReason, GenerationRequest, MessageText, ProviderErrorKind,
};
use agentkube_router::{
    ModelRouting, RouteDecision, RouterError, RoutingConstraints, RoutingRequest,
};
use agentkube_tasks::{TaskFailureKind, TaskResult, TaskUsage};
use std::{error::Error, fmt, future::Future, num::NonZeroU32, pin::Pin, sync::Arc};

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
            if let Some(max_tokens) = claim.task().spec().budget().max_tokens() {
                let value = u32::try_from(max_tokens.get())
                    .ok()
                    .and_then(NonZeroU32::new)
                    .ok_or(RuntimeError::OutputLimitTooLarge(max_tokens.get()))?;
                generation = generation.with_max_output_tokens(value);
            }
            let mut constraints = RoutingConstraints::new()
                .with_privacy(claim.task().spec().requirements().privacy());
            if let Some(maximum) = claim.task().spec().budget().max_cost_micro_usd() {
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
            | Self::MissingOutput => TaskFailureKind::Validation,
            Self::CostOverflow => TaskFailureKind::BudgetExceeded,
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
            Self::Unavailable => formatter.write_str("agent runtime is unavailable"),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Routing(error) => Some(error),
            _ => None,
        }
    }
}
