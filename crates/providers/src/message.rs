use agentkube_agents::ToolName;
use serde_json::Value;
use std::{error::Error, fmt};

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_TOOL_DESCRIPTION_BYTES: usize = 16 * 1024;
const MAX_TOOL_CALL_ID_BYTES: usize = 256;

/// Validated, non-empty text carried in a model conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageText(String);

impl MessageText {
    /// Creates bounded, non-blank message text.
    pub fn new(value: impl Into<String>) -> Result<Self, MessageError> {
        let value = value.into();
        validate_text("message content", &value, MAX_MESSAGE_BYTES)?;
        Ok(Self(value))
    }

    /// Returns the original text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque identifier correlating a tool result with a model tool call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolCallId(String);

impl ToolCallId {
    /// Creates a bounded, non-blank call identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, MessageError> {
        let value = value.into();
        validate_text("tool call ID", &value, MAX_TOOL_CALL_ID_BYTES)?;
        Ok(Self(value))
    }

    /// Returns the provider-issued identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Structured function call requested by a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    id: ToolCallId,
    name: ToolName,
    arguments: Value,
}

impl ToolCall {
    /// Creates a call whose arguments must be a JSON object.
    pub fn new(id: ToolCallId, name: ToolName, arguments: Value) -> Result<Self, MessageError> {
        if !arguments.is_object() {
            return Err(MessageError::ToolArgumentsNotObject);
        }
        Ok(Self {
            id,
            name,
            arguments,
        })
    }

    /// Returns the call identifier.
    #[must_use]
    pub const fn id(&self) -> &ToolCallId {
        &self.id
    }

    /// Returns the requested tool name.
    #[must_use]
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    /// Returns the structured tool arguments.
    #[must_use]
    pub const fn arguments(&self) -> &Value {
        &self.arguments
    }
}

/// Tool contract advertised to a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    name: ToolName,
    description: String,
    input_schema: Value,
}

impl ToolDefinition {
    /// Creates a tool definition with a JSON-object input schema.
    pub fn new(
        name: ToolName,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Result<Self, MessageError> {
        let description = description.into();
        validate_text("tool description", &description, MAX_TOOL_DESCRIPTION_BYTES)?;
        if !input_schema.is_object() {
            return Err(MessageError::ToolSchemaNotObject);
        }
        Ok(Self {
            name,
            description,
            input_schema,
        })
    }

    /// Returns the advertised tool name.
    #[must_use]
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    /// Returns the human-readable tool description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the provider-neutral JSON input schema.
    #[must_use]
    pub const fn input_schema(&self) -> &Value {
        &self.input_schema
    }
}

/// Role of a message in a model conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageRole {
    /// Control instructions supplied by the runtime.
    System,
    /// Input supplied on behalf of a user or task.
    User,
    /// Prior model output.
    Assistant,
    /// Result of a previously requested tool call.
    Tool,
}

/// Validated conversation message independent of provider wire formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    role: MessageRole,
    content: Option<MessageText>,
    tool_calls: Vec<ToolCall>,
    tool_call_id: Option<ToolCallId>,
}

impl ChatMessage {
    /// Creates a system message.
    #[must_use]
    pub const fn system(content: MessageText) -> Self {
        Self::text(MessageRole::System, content)
    }

    /// Creates a user message.
    #[must_use]
    pub const fn user(content: MessageText) -> Self {
        Self::text(MessageRole::User, content)
    }

    /// Creates an assistant text message.
    #[must_use]
    pub const fn assistant(content: MessageText) -> Self {
        Self::text(MessageRole::Assistant, content)
    }

    /// Creates an assistant message requesting one or more tool calls.
    pub fn assistant_tool_calls(
        content: Option<MessageText>,
        tool_calls: Vec<ToolCall>,
    ) -> Result<Self, MessageError> {
        if tool_calls.is_empty() {
            return Err(MessageError::MissingToolCalls);
        }
        Ok(Self {
            role: MessageRole::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        })
    }

    /// Creates a tool-result message.
    #[must_use]
    pub fn tool(call_id: ToolCallId, content: MessageText) -> Self {
        Self {
            role: MessageRole::Tool,
            content: Some(content),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id),
        }
    }

    /// Returns the message role.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Returns text content when present.
    #[must_use]
    pub const fn content(&self) -> Option<&MessageText> {
        self.content.as_ref()
    }

    /// Returns tool calls emitted by an assistant message.
    #[must_use]
    pub fn tool_calls(&self) -> &[ToolCall] {
        &self.tool_calls
    }

    /// Returns the correlated call identifier for a tool result.
    #[must_use]
    pub const fn tool_call_id(&self) -> Option<&ToolCallId> {
        self.tool_call_id.as_ref()
    }

    const fn text(role: MessageRole, content: MessageText) -> Self {
        Self {
            role,
            content: Some(content),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
}

/// Invalid provider-neutral message or tool definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageError {
    /// Required text is blank.
    EmptyText(&'static str),
    /// Text exceeds the protocol-independent safety limit.
    TextTooLong(&'static str),
    /// Tool-call arguments are not a JSON object.
    ToolArgumentsNotObject,
    /// A tool input schema is not a JSON object.
    ToolSchemaNotObject,
    /// An assistant tool-call message contains no calls.
    MissingToolCalls,
}

impl fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText(field) => write!(formatter, "{field} must not be blank"),
            Self::TextTooLong(field) => write!(formatter, "{field} exceeds its size limit"),
            Self::ToolArgumentsNotObject => {
                formatter.write_str("tool-call arguments must be a JSON object")
            }
            Self::ToolSchemaNotObject => {
                formatter.write_str("tool input schema must be a JSON object")
            }
            Self::MissingToolCalls => {
                formatter.write_str("assistant tool-call message needs at least one call")
            }
        }
    }
}

impl Error for MessageError {}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), MessageError> {
    if value.trim().is_empty() {
        Err(MessageError::EmptyText(field))
    } else if value.len() > maximum {
        Err(MessageError::TextTooLong(field))
    } else {
        Ok(())
    }
}
