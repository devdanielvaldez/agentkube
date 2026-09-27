//! Stable wire contracts used by AgentKube APIs, clients, and event streams.
//!
//! The protocol crate owns serialization shapes and validates values at the
//! system boundary. It intentionally contains no transport, persistence, or
//! domain orchestration logic.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod api_version;
mod document;
mod error;
mod kind;
mod list;
mod watch;

pub use api_version::{ApiVersion, ApiVersionError, ApiVersionErrorKind};
pub use document::{ResourceDocument, TypeMeta};
pub use error::{
    ApiError, ApiErrorDetails, ApiErrorReason, ApiStatusCode, FieldViolation, InvalidStatusCode,
};
pub use kind::{ResourceKind, ResourceKindError, ResourceKindErrorKind};
pub use list::{ContinueToken, ContinueTokenError, ListMetadata, ListOptions, ListResponse};
pub use watch::{WatchEvent, WatchEventType};
