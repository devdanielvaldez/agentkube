use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, str::FromStr};

const MAX_KIND_LENGTH: usize = 63;

/// A validated PascalCase resource kind such as `AgentTask`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceKind(String);

impl ResourceKind {
    /// Parses and validates a resource kind.
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceKindError> {
        let value = value.into();
        validate(&value)?;
        Ok(Self(value))
    }

    /// Returns the kind as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the kind and returns its string representation.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ResourceKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(formatter)
    }
}

impl FromStr for ResourceKind {
    type Err = ResourceKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for ResourceKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ResourceKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Error returned when parsing a [`ResourceKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceKindError {
    input: String,
    kind: ResourceKindErrorKind,
}

impl ResourceKindError {
    fn new(input: &str, kind: ResourceKindErrorKind) -> Self {
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
    pub const fn kind(&self) -> ResourceKindErrorKind {
        self.kind
    }
}

impl fmt::Display for ResourceKindError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid resource kind {:?}: {}",
            self.input, self.kind
        )
    }
}

impl Error for ResourceKindError {}

/// Category of a [`ResourceKind`] validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKindErrorKind {
    /// The value is empty.
    Empty,
    /// The value exceeds the maximum protocol length.
    TooLong,
    /// The value does not begin with an uppercase ASCII letter.
    InvalidStart,
    /// The value contains a non-alphanumeric character.
    InvalidCharacter,
}

impl fmt::Display for ResourceKindErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "must not be empty",
            Self::TooLong => "is too long",
            Self::InvalidStart => "must start with an uppercase ASCII letter",
            Self::InvalidCharacter => "must contain only ASCII alphanumeric characters",
        };
        formatter.write_str(message)
    }
}

fn validate(value: &str) -> Result<(), ResourceKindError> {
    if value.is_empty() {
        return Err(ResourceKindError::new(value, ResourceKindErrorKind::Empty));
    }
    if value.len() > MAX_KIND_LENGTH {
        return Err(ResourceKindError::new(
            value,
            ResourceKindErrorKind::TooLong,
        ));
    }
    if !value.as_bytes()[0].is_ascii_uppercase() {
        return Err(ResourceKindError::new(
            value,
            ResourceKindErrorKind::InvalidStart,
        ));
    }
    if !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(ResourceKindError::new(
            value,
            ResourceKindErrorKind::InvalidCharacter,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_pascal_case_kinds() {
        for value in ["Agent", "AgentTask", "AgentCronJob", "V1Model"] {
            assert!(ResourceKind::new(value).is_ok());
        }
    }

    #[test]
    fn rejects_malformed_kinds() {
        for value in ["", "agent", "Agent-Task", "Ágent"] {
            assert!(ResourceKind::new(value).is_err());
        }
    }
}
