use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, str::FromStr};

const MAX_API_VERSION_LENGTH: usize = 269;
const MAX_GROUP_LENGTH: usize = 253;
const MAX_LABEL_LENGTH: usize = 63;

/// A validated AgentKube API version such as `agentkube.ai/v1`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApiVersion(String);

impl ApiVersion {
    /// Parses an API version in `<group>/<version>` form.
    pub fn new(value: impl Into<String>) -> Result<Self, ApiVersionError> {
        let value = value.into();
        validate(&value)?;
        Ok(Self(value))
    }

    /// Returns the complete serialized API version.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the API group, for example `agentkube.ai`.
    #[must_use]
    pub fn group(&self) -> &str {
        self.0
            .split_once('/')
            .map(|(group, _)| group)
            .expect("ApiVersion always contains a validated separator")
    }

    /// Returns the version component, for example `v1beta1`.
    #[must_use]
    pub fn version(&self) -> &str {
        self.0
            .split_once('/')
            .map(|(_, version)| version)
            .expect("ApiVersion always contains a validated separator")
    }

    /// Consumes the API version and returns its string representation.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ApiVersion {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(formatter)
    }
}

impl FromStr for ApiVersion {
    type Err = ApiVersionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for ApiVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ApiVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Error returned when parsing an [`ApiVersion`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiVersionError {
    input: String,
    kind: ApiVersionErrorKind,
}

impl ApiVersionError {
    fn new(input: &str, kind: ApiVersionErrorKind) -> Self {
        Self {
            input: input.to_owned(),
            kind,
        }
    }

    /// Returns the invalid input.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the category of validation failure.
    #[must_use]
    pub const fn kind(&self) -> ApiVersionErrorKind {
        self.kind
    }
}

impl fmt::Display for ApiVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid API version {:?}: {}",
            self.input, self.kind
        )
    }
}

impl Error for ApiVersionError {}

/// Category of an [`ApiVersion`] validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiVersionErrorKind {
    /// The complete value is empty.
    Empty,
    /// The value is longer than the protocol permits.
    TooLong,
    /// The required group/version separator is missing or repeated.
    InvalidStructure,
    /// The group is not a valid lowercase DNS subdomain.
    InvalidGroup,
    /// The version is not `vN`, `vNalphaN`, or `vNbetaN`.
    InvalidVersion,
}

impl fmt::Display for ApiVersionErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "must not be empty",
            Self::TooLong => "is too long",
            Self::InvalidStructure => "must contain exactly one '/' separator",
            Self::InvalidGroup => "group must be a lowercase DNS subdomain",
            Self::InvalidVersion => "version must match vN, vNalphaN, or vNbetaN",
        };
        formatter.write_str(message)
    }
}

fn validate(value: &str) -> Result<(), ApiVersionError> {
    if value.is_empty() {
        return Err(ApiVersionError::new(value, ApiVersionErrorKind::Empty));
    }
    if value.len() > MAX_API_VERSION_LENGTH {
        return Err(ApiVersionError::new(value, ApiVersionErrorKind::TooLong));
    }

    let Some((group, version)) = value.split_once('/') else {
        return Err(ApiVersionError::new(
            value,
            ApiVersionErrorKind::InvalidStructure,
        ));
    };
    if version.contains('/') {
        return Err(ApiVersionError::new(
            value,
            ApiVersionErrorKind::InvalidStructure,
        ));
    }
    if !is_dns_subdomain(group) {
        return Err(ApiVersionError::new(
            value,
            ApiVersionErrorKind::InvalidGroup,
        ));
    }
    if !is_kubernetes_version(version) {
        return Err(ApiVersionError::new(
            value,
            ApiVersionErrorKind::InvalidVersion,
        ));
    }

    Ok(())
}

fn is_dns_subdomain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_GROUP_LENGTH
        && value
            .split('.')
            .all(|label| is_dns_label(label) && label.len() <= MAX_LABEL_LENGTH)
}

fn is_dns_label(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    let Some(last) = bytes.last() else {
        return false;
    };
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();

    is_alphanumeric(*first)
        && is_alphanumeric(*last)
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || *byte == b'-')
}

fn is_kubernetes_version(value: &str) -> bool {
    let Some(remainder) = value.strip_prefix('v') else {
        return false;
    };
    let stable_digits = remainder.bytes().take_while(u8::is_ascii_digit).count();
    if stable_digits == 0 {
        return false;
    }

    let suffix = &remainder[stable_digits..];
    if suffix.is_empty() {
        return true;
    }

    ["alpha", "beta"].into_iter().any(|stage| {
        suffix.strip_prefix(stage).is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_stable_and_prerelease_versions() {
        for value in [
            "agentkube.ai/v1",
            "agentkube.ai/v2alpha1",
            "tasks.agentkube.ai/v10beta2",
        ] {
            assert!(ApiVersion::new(value).is_ok(), "{value} should be valid");
        }
    }

    #[test]
    fn rejects_invalid_versions() {
        for value in [
            "",
            "v1",
            "AgentKube.ai/v1",
            "agentkube.ai/1",
            "agentkube.ai/v1rc1",
            "agentkube.ai/v1/extra",
        ] {
            assert!(ApiVersion::new(value).is_err(), "{value:?} should fail");
        }
    }

    #[test]
    fn exposes_group_and_version() {
        let version = ApiVersion::new("agentkube.ai/v1beta2").unwrap();

        assert_eq!(version.group(), "agentkube.ai");
        assert_eq!(version.version(), "v1beta2");
    }

    #[test]
    fn deserialization_preserves_validation() {
        assert!(serde_json::from_str::<ApiVersion>(r#""invalid""#).is_err());
    }
}
