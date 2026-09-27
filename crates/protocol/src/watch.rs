use crate::ApiError;
use serde::{Deserialize, Serialize};

/// Change notification emitted by a resource watch stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "object", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WatchEvent<T> {
    /// A resource entered the watched collection.
    Added(T),
    /// A watched resource changed.
    Modified(T),
    /// A resource left the watched collection.
    Deleted(T),
    /// A progress marker carrying the most recent resource version.
    Bookmark(T),
    /// A terminal or recoverable watch failure.
    Error(ApiError),
}

impl<T> WatchEvent<T> {
    /// Returns the event category without inspecting its payload.
    #[must_use]
    pub const fn event_type(&self) -> WatchEventType {
        match self {
            Self::Added(_) => WatchEventType::Added,
            Self::Modified(_) => WatchEventType::Modified,
            Self::Deleted(_) => WatchEventType::Deleted,
            Self::Bookmark(_) => WatchEventType::Bookmark,
            Self::Error(_) => WatchEventType::Error,
        }
    }

    /// Maps resource payloads while preserving errors.
    #[must_use]
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> WatchEvent<U> {
        match self {
            Self::Added(value) => WatchEvent::Added(map(value)),
            Self::Modified(value) => WatchEvent::Modified(map(value)),
            Self::Deleted(value) => WatchEvent::Deleted(map(value)),
            Self::Bookmark(value) => WatchEvent::Bookmark(map(value)),
            Self::Error(error) => WatchEvent::Error(error),
        }
    }
}

/// Payload-independent watch event category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchEventType {
    /// Resource addition.
    Added,
    /// Resource modification.
    Modified,
    /// Resource deletion.
    Deleted,
    /// Stream progress bookmark.
    Bookmark,
    /// Watch failure.
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApiErrorReason, ApiStatusCode};

    #[test]
    fn serializes_events_to_a_stable_envelope() {
        let value = serde_json::to_value(WatchEvent::Added("task-1")).unwrap();

        assert_eq!(
            value,
            serde_json::json!({"type": "ADDED", "object": "task-1"})
        );
    }

    #[test]
    fn map_preserves_error_events() {
        let event: WatchEvent<u8> = WatchEvent::Error(ApiError::new(
            ApiStatusCode::new(503).unwrap(),
            ApiErrorReason::ProviderUnavailable,
            "provider unavailable",
        ));

        let mapped = event.map(u16::from);

        assert_eq!(mapped.event_type(), WatchEventType::Error);
    }
}
