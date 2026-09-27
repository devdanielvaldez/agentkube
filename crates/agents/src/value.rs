use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, str::FromStr};

const MAX_SLUG_LENGTH: usize = 63;
const MAX_MODEL_NAME_LENGTH: usize = 128;
const MAX_INSTRUCTIONS_LENGTH: usize = 64 * 1024;

/// Category of a validated agent-domain value failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainValueErrorKind {
    /// The value is empty or only whitespace.
    Empty,
    /// The value exceeds its protocol size limit.
    TooLong,
    /// The value begins or ends with an unsupported character.
    InvalidBoundary,
    /// The value contains an unsupported character.
    InvalidCharacter,
}

impl fmt::Display for DomainValueErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "must not be empty",
            Self::TooLong => "is too long",
            Self::InvalidBoundary => "must start and end with an alphanumeric character",
            Self::InvalidCharacter => "contains an unsupported character",
        };
        formatter.write_str(message)
    }
}

/// Error returned when an agent-domain string violates its invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainValueError {
    field: &'static str,
    input: String,
    kind: DomainValueErrorKind,
}

impl DomainValueError {
    fn new(field: &'static str, input: &str, kind: DomainValueErrorKind) -> Self {
        Self {
            field,
            input: input.to_owned(),
            kind,
        }
    }

    /// Returns the logical field name.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the rejected input.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the category of validation failure.
    #[must_use]
    pub const fn kind(&self) -> DomainValueErrorKind {
        self.kind
    }
}

impl fmt::Display for DomainValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} {:?}: {}",
            self.field, self.input, self.kind
        )
    }
}

impl Error for DomainValueError {}

macro_rules! string_value {
    ($name:ident, $field:literal, $description:literal, $validator:ident) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Parses and validates the value.
            pub fn new(value: impl Into<String>) -> Result<Self, DomainValueError> {
                let value = value.into();
                $validator($field, &value)?;
                Ok(Self(value))
            }

            /// Returns the validated string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the value and returns its string representation.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.as_str().fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = DomainValueError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
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

string_value!(
    AgentRole,
    "role",
    "Validated role advertised by an agent, such as `software-engineer`.",
    validate_slug
);
string_value!(
    Capability,
    "capability",
    "Validated capability used for scheduler matching.",
    validate_slug
);
string_value!(
    ToolName,
    "tool",
    "Validated name of a tool requested by an agent.",
    validate_slug
);
string_value!(
    ProviderName,
    "provider",
    "Validated model-provider name.",
    validate_slug
);
string_value!(
    ModelName,
    "model",
    "Validated provider-specific model name.",
    validate_model_name
);

/// Validated system instructions associated with an agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Instructions(String);

impl Instructions {
    /// Creates non-empty instructions no larger than 64 KiB.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainValueError::new(
                "instructions",
                &value,
                DomainValueErrorKind::Empty,
            ));
        }
        if value.len() > MAX_INSTRUCTIONS_LENGTH {
            return Err(DomainValueError::new(
                "instructions",
                &value,
                DomainValueErrorKind::TooLong,
            ));
        }
        Ok(Self(value))
    }

    /// Returns the instructions without altering whitespace.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the instructions and returns the original string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for Instructions {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Instructions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(formatter)
    }
}

impl Serialize for Instructions {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Instructions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

fn validate_slug(field: &'static str, value: &str) -> Result<(), DomainValueError> {
    validate_bounded(field, value, MAX_SLUG_LENGTH)?;
    let bytes = value.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !is_alphanumeric(bytes[0]) || !is_alphanumeric(bytes[bytes.len() - 1]) {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::InvalidBoundary,
        ));
    }
    if !bytes
        .iter()
        .all(|byte| is_alphanumeric(*byte) || matches!(*byte, b'-' | b'.'))
    {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::InvalidCharacter,
        ));
    }
    Ok(())
}

fn validate_model_name(field: &'static str, value: &str) -> Result<(), DomainValueError> {
    validate_bounded(field, value, MAX_MODEL_NAME_LENGTH)?;
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::InvalidBoundary,
        ));
    }
    if !bytes.iter().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b':' | b'/')
    }) {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::InvalidCharacter,
        ));
    }
    Ok(())
}

fn validate_bounded(
    field: &'static str,
    value: &str,
    max_length: usize,
) -> Result<(), DomainValueError> {
    if value.is_empty() {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::Empty,
        ));
    }
    if value.len() > max_length {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::TooLong,
        ));
    }
    if !value.is_ascii() {
        return Err(DomainValueError::new(
            field,
            value,
            DomainValueErrorKind::InvalidCharacter,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_domain_names_and_provider_model_names() {
        assert!(AgentRole::new("software-engineer").is_ok());
        assert!(Capability::new("tool-calling").is_ok());
        assert!(ProviderName::new("openai").is_ok());
        assert!(ModelName::new("provider/model-v1.2").is_ok());
    }

    #[test]
    fn rejects_values_that_would_be_ambiguous_on_the_wire() {
        assert!(AgentRole::new("Software Engineer").is_err());
        assert!(Capability::new("-coding").is_err());
        assert!(ToolName::new("github/").is_err());
    }

    #[test]
    fn deserialization_preserves_instructions_invariants() {
        assert!(serde_json::from_str::<Instructions>(r#""   ""#).is_err());
    }
}
