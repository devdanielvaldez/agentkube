//! Human-readable positive durations shared by declarative resources and runtime configuration.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, str::FromStr, time::Duration};

/// Positive duration serialized in a compact human-readable form such as `30s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HumanDuration(Duration);

impl HumanDuration {
    /// Creates a positive duration from milliseconds.
    pub fn from_millis(milliseconds: u64) -> Result<Self, DurationError> {
        if milliseconds == 0 {
            Err(DurationError::new("0ms", DurationErrorKind::Zero))
        } else {
            Ok(Self(Duration::from_millis(milliseconds)))
        }
    }

    /// Returns the standard-library duration.
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }

    /// Returns the duration in whole milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> u128 {
        self.0.as_millis()
    }
}

impl fmt::Display for HumanDuration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let milliseconds = self.0.as_millis();
        const HOUR_MS: u128 = 3_600_000;
        const MINUTE_MS: u128 = 60_000;
        const SECOND_MS: u128 = 1_000;

        if milliseconds % HOUR_MS == 0 {
            write!(formatter, "{}h", milliseconds / HOUR_MS)
        } else if milliseconds % MINUTE_MS == 0 {
            write!(formatter, "{}m", milliseconds / MINUTE_MS)
        } else if milliseconds % SECOND_MS == 0 {
            write!(formatter, "{}s", milliseconds / SECOND_MS)
        } else {
            write!(formatter, "{milliseconds}ms")
        }
    }
}

impl FromStr for HumanDuration {
    type Err = DurationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let digit_count = value.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            return Err(DurationError::new(value, DurationErrorKind::InvalidNumber));
        }

        let amount = value[..digit_count]
            .parse::<u64>()
            .map_err(|_| DurationError::new(value, DurationErrorKind::InvalidNumber))?;
        if amount == 0 {
            return Err(DurationError::new(value, DurationErrorKind::Zero));
        }

        let multiplier = match &value[digit_count..] {
            "ms" => 1,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            _ => return Err(DurationError::new(value, DurationErrorKind::InvalidUnit)),
        };
        let milliseconds = amount
            .checked_mul(multiplier)
            .ok_or_else(|| DurationError::new(value, DurationErrorKind::Overflow))?;

        Ok(Self(Duration::from_millis(milliseconds)))
    }
}

impl Serialize for HumanDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for HumanDuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// Error returned when parsing a [`HumanDuration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationError {
    input: String,
    kind: DurationErrorKind,
}

impl DurationError {
    fn new(input: &str, kind: DurationErrorKind) -> Self {
        Self {
            input: input.to_owned(),
            kind,
        }
    }

    /// Returns the rejected value.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the category of parse failure.
    #[must_use]
    pub const fn kind(&self) -> DurationErrorKind {
        self.kind
    }
}

impl fmt::Display for DurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid duration {:?}: {}",
            self.input, self.kind
        )
    }
}

impl Error for DurationError {}

/// Category of a human-duration parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationErrorKind {
    /// The numeric component is absent or malformed.
    InvalidNumber,
    /// The duration is zero, but timeouts and intervals must be positive.
    Zero,
    /// The suffix is not one of `ms`, `s`, `m`, or `h`.
    InvalidUnit,
    /// Conversion to milliseconds exceeded the supported range.
    Overflow,
}

impl fmt::Display for DurationErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidNumber => "expected an unsigned integer followed by a unit",
            Self::Zero => "must be greater than zero",
            Self::InvalidUnit => "unit must be one of ms, s, m, or h",
            Self::Overflow => "value exceeds the supported duration range",
        };
        formatter.write_str(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_units() {
        assert_eq!("250ms".parse::<HumanDuration>().unwrap().as_millis(), 250);
        assert_eq!("30s".parse::<HumanDuration>().unwrap().as_millis(), 30_000);
        assert_eq!("5m".parse::<HumanDuration>().unwrap().as_millis(), 300_000);
        assert_eq!(
            "2h".parse::<HumanDuration>().unwrap().as_millis(),
            7_200_000
        );
    }

    #[test]
    fn rejects_zero_malformed_and_overflowing_values() {
        for value in ["0s", "-1s", "1d", "s", "18446744073709551615h"] {
            assert!(
                value.parse::<HumanDuration>().is_err(),
                "{value} should fail"
            );
        }
    }

    #[test]
    fn serialization_uses_the_largest_exact_unit() {
        let duration = HumanDuration::from_millis(120_000).unwrap();
        assert_eq!(serde_json::to_string(&duration).unwrap(), r#""2m""#);
    }

    #[test]
    fn invalid_json_value_cannot_bypass_validation() {
        assert!(serde_json::from_str::<HumanDuration>(r#""0ms""#).is_err());
    }
}
