//! Provider-neutral tool registration and execution contracts.

use agentkube_agents::ToolName;
use agentkube_providers::ToolDefinition;
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, future::Future, pin::Pin, sync::Arc};

/// Sendable future returned by a tool implementation.
pub type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, ToolExecutionError>> + Send + 'a>>;

/// One executable tool advertised to model providers.
pub trait ToolExecutor: Send + Sync {
    /// Returns the provider-neutral schema advertised to the model.
    fn definition(&self) -> &ToolDefinition;

    /// Executes a validated JSON-object argument payload.
    fn execute<'a>(&'a self, arguments: &'a Value) -> ToolFuture<'a>;
}

/// Immutable registry used by an agent runtime.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<BTreeMap<ToolName, Arc<dyn ToolExecutor>>>,
}

impl ToolRegistry {
    /// Builds a registry, rejecting duplicate tool names.
    pub fn new(tools: Vec<Arc<dyn ToolExecutor>>) -> Result<Self, ToolRegistryError> {
        let mut registered = BTreeMap::new();
        for tool in tools {
            let name = tool.definition().name().clone();
            if registered.insert(name.clone(), tool).is_some() {
                return Err(ToolRegistryError::Duplicate(name));
            }
        }
        Ok(Self {
            tools: Arc::new(registered),
        })
    }

    /// Resolves the complete schemas requested by an agent definition.
    pub fn definitions<'a>(
        &'a self,
        names: impl IntoIterator<Item = &'a ToolName>,
    ) -> Result<Vec<ToolDefinition>, ToolRegistryError> {
        names
            .into_iter()
            .map(|name| {
                self.tools
                    .get(name)
                    .map(|tool| tool.definition().clone())
                    .ok_or_else(|| ToolRegistryError::Missing(name.clone()))
            })
            .collect()
    }

    /// Executes a registered tool. Callers must separately enforce whether
    /// the active agent was granted this tool.
    pub async fn execute(
        &self,
        name: &ToolName,
        arguments: &Value,
    ) -> Result<String, ToolRegistryError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolRegistryError::Missing(name.clone()))?;
        tool.execute(arguments)
            .await
            .map_err(|source| ToolRegistryError::Execution {
                tool: name.clone(),
                source,
            })
    }
}

/// Safe tool failure surfaced to the model runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionError(String);

impl ToolExecutionError {
    /// Creates an error whose message must not contain credentials or raw
    /// environment values.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ToolExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ToolExecutionError {}

/// Invalid registry lookup or tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRegistryError {
    /// Two executors advertised the same name.
    Duplicate(ToolName),
    /// The agent requested an unregistered tool.
    Missing(ToolName),
    /// A registered tool returned a safe execution failure.
    Execution {
        /// Tool that failed.
        tool: ToolName,
        /// Sanitized failure.
        source: ToolExecutionError,
    },
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(name) => write!(formatter, "duplicate tool {name}"),
            Self::Missing(name) => write!(formatter, "tool {name} is not registered"),
            Self::Execution { tool, source } => write!(formatter, "tool {tool} failed: {source}"),
        }
    }
}

impl Error for ToolRegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Execution { source, .. } => Some(source),
            Self::Duplicate(_) | Self::Missing(_) => None,
        }
    }
}
