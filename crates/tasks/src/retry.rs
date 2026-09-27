use crate::TaskFailureKind;
use agentkube_core::HumanDuration;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroU16};

/// Delay policy applied between task attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(tag = "strategy", rename_all = "camelCase")]
pub enum BackoffPolicy {
    /// Retry immediately.
    #[default]
    None,
    /// Wait the same delay before every retry.
    Fixed {
        /// Delay between attempts.
        delay: HumanDuration,
    },
    /// Double the delay after each failure, capped at a maximum.
    Exponential {
        /// Delay before the first retry.
        initial: HumanDuration,
        /// Maximum delay between attempts.
        maximum: HumanDuration,
    },
}

impl BackoffPolicy {
    /// Creates exponential backoff with a valid maximum.
    pub fn exponential(
        initial: HumanDuration,
        maximum: HumanDuration,
    ) -> Result<Self, BackoffPolicyError> {
        if initial > maximum {
            Err(BackoffPolicyError::InitialExceedsMaximum)
        } else {
            Ok(Self::Exponential { initial, maximum })
        }
    }

    /// Calculates the delay before a one-based retry number.
    #[must_use]
    pub fn delay_for_retry(self, retry_number: NonZeroU16) -> Option<HumanDuration> {
        match self {
            Self::None => None,
            Self::Fixed { delay } => Some(delay),
            Self::Exponential { initial, maximum } => {
                let exponent = u32::from(retry_number.get() - 1).min(63);
                let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
                let initial_ms = u64::try_from(initial.as_millis()).unwrap_or(u64::MAX);
                let maximum_ms = u64::try_from(maximum.as_millis()).unwrap_or(u64::MAX);
                let milliseconds = initial_ms.saturating_mul(multiplier).min(maximum_ms);
                Some(
                    HumanDuration::from_millis(milliseconds)
                        .expect("positive backoff inputs produce a positive delay"),
                )
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "strategy", rename_all = "camelCase")]
enum BackoffPolicyWire {
    None,
    Fixed {
        delay: HumanDuration,
    },
    Exponential {
        initial: HumanDuration,
        maximum: HumanDuration,
    },
}

impl<'de> Deserialize<'de> for BackoffPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match BackoffPolicyWire::deserialize(deserializer)? {
            BackoffPolicyWire::None => Ok(Self::None),
            BackoffPolicyWire::Fixed { delay } => Ok(Self::Fixed { delay }),
            BackoffPolicyWire::Exponential { initial, maximum } => {
                Self::exponential(initial, maximum).map_err(de::Error::custom)
            }
        }
    }
}

/// Invalid backoff parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffPolicyError {
    /// Initial delay is longer than the configured maximum.
    InitialExceedsMaximum,
}

impl fmt::Display for BackoffPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("initial backoff delay must not exceed maximum delay")
    }
}

impl Error for BackoffPolicyError {}

/// Retry behavior applied after a task failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryPolicy {
    max_attempts: NonZeroU16,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    retry_on: BTreeSet<TaskFailureKind>,
    #[serde(default)]
    backoff: BackoffPolicy,
}

impl RetryPolicy {
    /// Creates a policy with a total attempt ceiling, including the first attempt.
    #[must_use]
    pub fn new(max_attempts: NonZeroU16) -> Self {
        Self {
            max_attempts,
            retry_on: BTreeSet::new(),
            backoff: BackoffPolicy::None,
        }
    }

    /// Marks a failure category as retryable.
    #[must_use]
    pub fn with_retry_on(mut self, kind: TaskFailureKind) -> Self {
        self.retry_on.insert(kind);
        self
    }

    /// Applies retry delay behavior.
    #[must_use]
    pub const fn with_backoff(mut self, backoff: BackoffPolicy) -> Self {
        self.backoff = backoff;
        self
    }

    /// Returns whether a failure may start another attempt.
    #[must_use]
    pub fn permits(&self, attempts_started: u16, kind: TaskFailureKind) -> bool {
        attempts_started < self.max_attempts.get() && self.retry_on.contains(&kind)
    }

    /// Returns the total attempt ceiling.
    #[must_use]
    pub const fn max_attempts(&self) -> NonZeroU16 {
        self.max_attempts
    }

    /// Returns retryable failure categories.
    #[must_use]
    pub const fn retry_on(&self) -> &BTreeSet<TaskFailureKind> {
        &self.retry_on
    }

    /// Returns delay behavior.
    #[must_use]
    pub const fn backoff(&self) -> BackoffPolicy {
        self.backoff
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(NonZeroU16::new(1).expect("one is non-zero"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_doubles_and_caps() {
        let backoff =
            BackoffPolicy::exponential("1s".parse().unwrap(), "5s".parse().unwrap()).unwrap();

        assert_eq!(
            backoff
                .delay_for_retry(NonZeroU16::new(1).unwrap())
                .unwrap()
                .to_string(),
            "1s"
        );
        assert_eq!(
            backoff
                .delay_for_retry(NonZeroU16::new(3).unwrap())
                .unwrap()
                .to_string(),
            "4s"
        );
        assert_eq!(
            backoff
                .delay_for_retry(NonZeroU16::new(4).unwrap())
                .unwrap()
                .to_string(),
            "5s"
        );
    }

    #[test]
    fn invalid_exponential_backoff_is_rejected_during_deserialization() {
        let json = r#"{"strategy":"exponential","initial":"10s","maximum":"1s"}"#;
        assert!(serde_json::from_str::<BackoffPolicy>(json).is_err());
    }
}
