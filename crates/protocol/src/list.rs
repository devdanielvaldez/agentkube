use crate::TypeMeta;
use agentkube_core::ResourceVersion;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, num::NonZeroU32};

const MAX_CONTINUE_TOKEN_LENGTH: usize = 4096;

/// Opaque token used to continue a paginated list operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContinueToken(String);

impl ContinueToken {
    /// Validates a non-empty, bounded continuation token.
    pub fn new(value: impl Into<String>) -> Result<Self, ContinueTokenError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ContinueTokenError::Empty);
        }
        if value.len() > MAX_CONTINUE_TOKEN_LENGTH {
            return Err(ContinueTokenError::TooLong);
        }
        Ok(Self(value))
    }

    /// Returns the opaque token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the token and returns its encoded value.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Serialize for ContinueToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ContinueToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Validation failure for a [`ContinueToken`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinueTokenError {
    /// The token is empty.
    Empty,
    /// The token exceeds the protocol size limit.
    TooLong,
}

impl fmt::Display for ContinueTokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("continuation token must not be empty"),
            Self::TooLong => formatter.write_str("continuation token is too long"),
        }
    }
}

impl Error for ContinueTokenError {}

/// Query options for a consistent, paginated list operation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<NonZeroU32>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "continue")]
    continue_token: Option<ContinueToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_version: Option<ResourceVersion>,
}

impl ListOptions {
    /// Creates list options with no pagination constraints.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            limit: None,
            continue_token: None,
            resource_version: None,
        }
    }

    /// Sets the maximum number of resources returned in a page.
    #[must_use]
    pub const fn with_limit(mut self, limit: NonZeroU32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Continues a previous list operation.
    #[must_use]
    pub fn with_continue_token(mut self, token: ContinueToken) -> Self {
        self.continue_token = Some(token);
        self
    }

    /// Requests a view at a particular resource version.
    #[must_use]
    pub const fn with_resource_version(mut self, version: ResourceVersion) -> Self {
        self.resource_version = Some(version);
        self
    }

    /// Returns the requested page size.
    #[must_use]
    pub const fn limit(&self) -> Option<NonZeroU32> {
        self.limit
    }

    /// Returns the continuation token.
    #[must_use]
    pub const fn continue_token(&self) -> Option<&ContinueToken> {
        self.continue_token.as_ref()
    }

    /// Returns the requested consistency version.
    #[must_use]
    pub const fn resource_version(&self) -> Option<ResourceVersion> {
        self.resource_version
    }
}

/// Metadata accompanying a list response.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMetadata {
    /// Version at which the list was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_version: Option<ResourceVersion>,
    /// Token for retrieving the next page.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "continue")]
    continue_token: Option<ContinueToken>,
    /// Approximate number of items remaining after this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining_item_count: Option<u64>,
}

impl ListMetadata {
    /// Creates empty list metadata.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            resource_version: None,
            continue_token: None,
            remaining_item_count: None,
        }
    }

    /// Records the version at which the collection was observed.
    #[must_use]
    pub const fn with_resource_version(mut self, version: ResourceVersion) -> Self {
        self.resource_version = Some(version);
        self
    }

    /// Attaches the token required to fetch the next page.
    #[must_use]
    pub fn with_continue_token(mut self, token: ContinueToken) -> Self {
        self.continue_token = Some(token);
        self
    }

    /// Records the approximate number of resources remaining.
    #[must_use]
    pub const fn with_remaining_item_count(mut self, count: u64) -> Self {
        self.remaining_item_count = Some(count);
        self
    }

    /// Returns the observed collection version.
    #[must_use]
    pub const fn resource_version(&self) -> Option<ResourceVersion> {
        self.resource_version
    }

    /// Returns the next-page token.
    #[must_use]
    pub const fn continue_token(&self) -> Option<&ContinueToken> {
        self.continue_token.as_ref()
    }

    /// Returns the approximate number of remaining resources.
    #[must_use]
    pub const fn remaining_item_count(&self) -> Option<u64> {
        self.remaining_item_count
    }
}

/// Paginated collection returned by list endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse<T> {
    #[serde(flatten)]
    type_meta: TypeMeta,
    metadata: ListMetadata,
    items: Vec<T>,
}

impl<T> ListResponse<T> {
    /// Creates a list response.
    #[must_use]
    pub fn new(type_meta: TypeMeta, metadata: ListMetadata, items: Vec<T>) -> Self {
        Self {
            type_meta,
            metadata,
            items,
        }
    }

    /// Returns the schema identity for the list document.
    #[must_use]
    pub const fn type_meta(&self) -> &TypeMeta {
        &self.type_meta
    }

    /// Returns pagination and consistency metadata.
    #[must_use]
    pub const fn metadata(&self) -> &ListMetadata {
        &self.metadata
    }

    /// Returns the resources in this page.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Consumes the response and returns its resources.
    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApiVersion, ResourceKind};

    #[test]
    fn empty_options_serialize_without_null_fields() {
        assert_eq!(
            serde_json::to_value(ListOptions::new()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn pagination_uses_continue_as_the_wire_field() {
        let options = ListOptions::new()
            .with_limit(NonZeroU32::new(25).unwrap())
            .with_continue_token(ContinueToken::new("opaque-token").unwrap());
        let value = serde_json::to_value(options).unwrap();

        assert_eq!(value["limit"], 25);
        assert_eq!(value["continue"], "opaque-token");
    }

    #[test]
    fn empty_continue_token_is_rejected_during_deserialization() {
        assert!(serde_json::from_str::<ContinueToken>(r#"""#).is_err());
    }

    #[test]
    fn list_response_includes_type_and_page_metadata() {
        let response = ListResponse::new(
            TypeMeta::new(
                ApiVersion::new("agentkube.ai/v1").unwrap(),
                ResourceKind::new("AgentList").unwrap(),
            ),
            ListMetadata::new()
                .with_resource_version(ResourceVersion::new(7).unwrap())
                .with_remaining_item_count(2),
            vec!["agent-1"],
        );
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["apiVersion"], "agentkube.ai/v1");
        assert_eq!(value["kind"], "AgentList");
        assert_eq!(value["metadata"]["resourceVersion"], 7);
        assert_eq!(value["metadata"]["remainingItemCount"], 2);
        assert_eq!(value["items"], serde_json::json!(["agent-1"]));
    }
}
