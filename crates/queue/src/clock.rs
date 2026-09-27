use crate::QueueError;
use agentkube_core::HumanDuration;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Time source used to evaluate delayed delivery and lease expiration.
pub trait QueueClock: Send + Sync + 'static {
    /// Returns the current wall-clock instant.
    fn now(&self) -> Result<SystemTime, QueueError>;
}

/// Production queue clock backed by [`SystemTime`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl QueueClock for SystemClock {
    fn now(&self) -> Result<SystemTime, QueueError> {
        Ok(SystemTime::now())
    }
}

/// Deterministic, thread-safe clock for tests and simulations.
#[derive(Debug, Default)]
pub struct ManualClock {
    unix_milliseconds: AtomicU64,
}

impl ManualClock {
    /// Creates a clock at the Unix epoch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            unix_milliseconds: AtomicU64::new(0),
        }
    }

    /// Creates a clock at a specific millisecond offset from the Unix epoch.
    #[must_use]
    pub const fn at_unix_milliseconds(milliseconds: u64) -> Self {
        Self {
            unix_milliseconds: AtomicU64::new(milliseconds),
        }
    }

    /// Advances time atomically, rejecting values outside the supported range.
    pub fn advance(&self, duration: HumanDuration) -> Result<(), QueueError> {
        let increment =
            u64::try_from(duration.as_millis()).map_err(|_| QueueError::TimeOverflow)?;
        self.unix_milliseconds
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(increment)
            })
            .map(|_| ())
            .map_err(|_| QueueError::TimeOverflow)
    }
}

impl QueueClock for ManualClock {
    fn now(&self) -> Result<SystemTime, QueueError> {
        UNIX_EPOCH
            .checked_add(Duration::from_millis(
                self.unix_milliseconds.load(Ordering::SeqCst),
            ))
            .ok_or(QueueError::TimeOverflow)
    }
}
