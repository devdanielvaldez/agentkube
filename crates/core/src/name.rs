use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, str::FromStr};

const MAX_RESOURCE_NAME_LENGTH: usize = 253;
const MAX_NAMESPACE_LENGTH: usize = 63;

/// Describes why a user-provided domain value is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    field: &'static str,
    value: String,
    reason: &'static str,
}

impl ValidationError {
    fn new(field: &'static str, value: &str, reason: &'static str) -> Self {
        Self {
            field,
            value: value.to_owned(),
            reason,
        }
    }

    /// Returns the name of the invalid field.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the invalid value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns a human-readable explanation of the violated invariant.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} {:?}: {}",
            self.field, self.value, self.reason
        )
    }
}

impl Error for ValidationError {}

/// A DNS-1123 subdomain used as the name of an AgentKube resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceName(String);

impl ResourceName {
    /// Parses and validates a resource name.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        validate_dns_subdomain("resource name", &value, MAX_RESOURCE_NAME_LENGTH)?;
        Ok(Self(value))
    }

    /// Returns the resource name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name and returns its string representation.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// A DNS-1123 label that isolates AgentKube resources.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Namespace(String);

impl Namespace {
    /// Parses and validates a namespace.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        validate_dns_label("namespace", &value, MAX_NAMESPACE_LENGTH)?;
        Ok(Self(value))
    }

    /// Returns the namespace as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the namespace and returns its string representation.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Default for Namespace {
    fn default() -> Self {
        Self("default".to_owned())
    }
}

macro_rules! impl_string_value {
    ($type:ty) => {
        impl AsRef<str> for $type {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.as_str().fmt(formatter)
            }
        }

        impl FromStr for $type {
            type Err = ValidationError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

impl_string_value!(ResourceName);
impl_string_value!(Namespace);

fn validate_dns_subdomain(
    field: &'static str,
    value: &str,
    max_length: usize,
) -> Result<(), ValidationError> {
    if value.is_empty() {
        return Err(ValidationError::new(field, value, "must not be empty"));
    }
    if value.len() > max_length {
        return Err(ValidationError::new(field, value, "is too long"));
    }

    for label in value.split('.') {
        validate_dns_label(field, label, MAX_NAMESPACE_LENGTH)?;
    }

    Ok(())
}

fn validate_dns_label(
    field: &'static str,
    value: &str,
    max_length: usize,
) -> Result<(), ValidationError> {
    if value.is_empty() {
        return Err(ValidationError::new(field, value, "must not be empty"));
    }
    if value.len() > max_length {
        return Err(ValidationError::new(field, value, "is too long"));
    }
    if !value.is_ascii() {
        return Err(ValidationError::new(
            field,
            value,
            "must contain only ASCII characters",
        ));
    }

    let bytes = value.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();

    if !is_alphanumeric(bytes[0]) || !is_alphanumeric(bytes[bytes.len() - 1]) {
        return Err(ValidationError::new(
            field,
            value,
            "must start and end with a lowercase alphanumeric character",
        ));
    }
    if !bytes
        .iter()
        .all(|byte| is_alphanumeric(*byte) || *byte == b'-')
    {
        return Err(ValidationError::new(
            field,
            value,
            "may contain only lowercase alphanumeric characters or '-'",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_resource_names() {
        for value in [
            "backend",
            "backend-agent",
            "backend.agentkube.ai",
            "agent-1",
        ] {
            assert!(ResourceName::new(value).is_ok(), "{value} should be valid");
        }
    }

    #[test]
    fn rejects_invalid_resource_names() {
        for value in [
            "",
            "Backend",
            "-backend",
            "backend-",
            "backend_agent",
            "a..b",
        ] {
            assert!(
                ResourceName::new(value).is_err(),
                "{value:?} should be invalid"
            );
        }
    }

    #[test]
    fn namespace_defaults_to_default() {
        assert_eq!(Namespace::default().as_str(), "default");
    }

    #[test]
    fn rejects_subdomains_as_namespaces() {
        assert!(Namespace::new("engineering.production").is_err());
    }

    #[test]
    fn deserialization_preserves_validation() {
        let result = serde_json::from_str::<Namespace>(r#""Invalid""#);

        assert!(result.is_err());
    }
}
