use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt};

const MAX_OBJECTIVE_LENGTH: usize = 64 * 1024;

/// Validated objective assigned to an agent task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Objective(String);

impl Objective {
    /// Creates a non-empty objective no larger than 64 KiB.
    pub fn new(value: impl Into<String>) -> Result<Self, TaskValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(TaskValueError::new(
                "objective",
                value,
                TaskValueErrorKind::Empty,
            ));
        }
        if value.len() > MAX_OBJECTIVE_LENGTH {
            return Err(TaskValueError::new(
                "objective",
                value,
                TaskValueErrorKind::TooLong,
            ));
        }
        Ok(Self(value))
    }

    /// Returns the objective without modifying its formatting.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the objective and returns its original string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for Objective {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Objective {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(formatter)
    }
}

impl Serialize for Objective {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Objective {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Category of a task value validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskValueErrorKind {
    /// The value is empty or only whitespace.
    Empty,
    /// The value exceeds its protocol size limit.
    TooLong,
}

impl fmt::Display for TaskValueErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "must not be empty",
            Self::TooLong => "is too long",
        })
    }
}

/// Invalid user-provided task value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskValueError {
    field: &'static str,
    input: String,
    kind: TaskValueErrorKind,
}

impl TaskValueError {
    fn new(field: &'static str, input: String, kind: TaskValueErrorKind) -> Self {
        Self { field, input, kind }
    }

    /// Returns the field containing the invalid value.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the rejected value.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the validation failure category.
    #[must_use]
    pub const fn kind(&self) -> TaskValueErrorKind {
        self.kind
    }
}

impl fmt::Display for TaskValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} {:?}: {}",
            self.field, self.input, self.kind
        )
    }
}

impl Error for TaskValueError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objective_preserves_multiline_content() {
        let objective = Objective::new("Investigate the issue.\nFix it and add tests.").unwrap();
        assert_eq!(
            objective.as_str(),
            "Investigate the issue.\nFix it and add tests."
        );
    }

    #[test]
    fn objective_validation_survives_deserialization() {
        assert!(serde_json::from_str::<Objective>(r#""   ""#).is_err());
    }
}
