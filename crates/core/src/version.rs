use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt};

/// Monotonically increasing version used for optimistic concurrency control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ResourceVersion(u64);

impl ResourceVersion {
    /// Version assigned to a newly created resource.
    pub const INITIAL: Self = Self(1);

    /// Creates a resource version when the value is greater than zero.
    pub const fn new(value: u64) -> Result<Self, ResourceVersionError> {
        if value == 0 {
            Err(ResourceVersionError::Zero)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the numeric version.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next version, or an error if the counter is exhausted.
    pub const fn next(self) -> Result<Self, ResourceVersionError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(ResourceVersionError::Exhausted),
        }
    }
}

impl Default for ResourceVersion {
    fn default() -> Self {
        Self::INITIAL
    }
}

impl<'de> Deserialize<'de> for ResourceVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Error returned when a resource version violates its invariants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceVersionError {
    /// Resource versions start at one; zero is never valid.
    Zero,
    /// The version cannot be incremented because it reached `u64::MAX`.
    Exhausted,
}

impl fmt::Display for ResourceVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("resource version must be greater than zero"),
            Self::Exhausted => formatter.write_str("resource version counter is exhausted"),
        }
    }
}

impl Error for ResourceVersionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_version_starts_at_one() {
        assert_eq!(ResourceVersion::default().get(), 1);
    }

    #[test]
    fn next_increments_the_version() {
        assert_eq!(ResourceVersion::INITIAL.next().unwrap().get(), 2);
    }

    #[test]
    fn zero_is_rejected() {
        assert_eq!(ResourceVersion::new(0), Err(ResourceVersionError::Zero));
    }

    #[test]
    fn increment_reports_overflow() {
        let maximum = ResourceVersion::new(u64::MAX).unwrap();

        assert_eq!(maximum.next(), Err(ResourceVersionError::Exhausted));
    }

    #[test]
    fn deserialization_preserves_the_nonzero_invariant() {
        assert!(serde_json::from_str::<ResourceVersion>("0").is_err());
    }
}
