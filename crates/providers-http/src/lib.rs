//! HTTP model provider adapters for AgentKube.
//!
//! [`OllamaProvider`] talks to a local Ollama server and [`OpenAiProvider`]
//! talks to OpenAI or any OpenAI-compatible chat-completions endpoint. Both
//! implement [`ModelProvider`](agentkube_providers::ModelProvider) with real
//! requests, real usage accounting, and normalized errors; neither synthesizes
//! responses, identifiers, or token counts.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod anthropic;
mod gemini;
mod http;
mod ollama;
mod openai;

pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;
pub use http::AdapterError;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
