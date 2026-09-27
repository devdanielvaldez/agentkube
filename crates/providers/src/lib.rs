//! Provider-independent model inference contracts.
//!
//! Provider SDKs belong in adapters built on these types. This crate contains
//! no network client and does not expose vendor-specific request shapes.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod capabilities;
mod error;
mod message;
mod model;
mod provider;
mod scripted;

pub use capabilities::{
    CapabilityError, ModelCapabilities, ProviderCapabilities, ProviderHealth, ProviderHealthStatus,
};
pub use error::{ProviderError, ProviderErrorKind, ProviderFuture, ProviderResult};
pub use message::{
    ChatMessage, MessageError, MessageRole, MessageText, ToolCall, ToolCallId, ToolDefinition,
};
pub use model::{
    CostEstimate, FinishReason, GenerationRequest, GenerationRequestError, GenerationResponse,
    GenerationResponseError, GenerationUsage, StreamEvent,
};
pub use provider::{ModelEventStream, ModelProvider};
pub use scripted::ScriptedProvider;
