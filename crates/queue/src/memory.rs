use crate::{
    EnqueueRequest, LeaseId, QueueClock, QueueError, QueueFuture, QueueStats, SystemClock,
    TaskLease, TaskQueue,
};
use agentkube_core::{HumanDuration, NodeId, TaskId};
use agentkube_tasks::TaskPriority;
use std::{
    cmp::Reverse,
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::SystemTime,
};

/// Thread-safe in-memory implementation of [`TaskQueue`].
///
/// All queue indexes and state transitions share one lock, which makes
/// duplicate detection, leasing, settlement, and lease recovery atomic.
pub struct InMemoryTaskQueue<C = SystemClock> {
    clock: Arc<C>,
    inner: Arc<Mutex<QueueStore>>,
}

struct QueueStore {
    entries: HashMap<TaskId, QueueEntry>,
    leases: HashMap<LeaseId, TaskId>,
    next_sequence: Option<u64>,
}

struct QueueEntry {
    priority: TaskPriority,
    sequence: u64,
    delivery_attempts: u64,
    state: EntryState,
}

enum EntryState {
    Available {
        at: SystemTime,
    },
    Leased {
        lease_id: LeaseId,
        consumer: NodeId,
        deadline: SystemTime,
    },
}

impl Default for QueueStore {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            leases: HashMap::new(),
            next_sequence: Some(1),
        }
    }
}

impl QueueStore {
    fn allocate_sequence(&mut self) -> Result<u64, QueueError> {
        let sequence = self.next_sequence.ok_or(QueueError::SequenceExhausted)?;
        self.next_sequence = sequence.checked_add(1);
        Ok(sequence)
    }
}

impl<C> Clone for InMemoryTaskQueue<C> {
    fn clone(&self) -> Self {
        Self {
            clock: Arc::clone(&self.clock),
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for InMemoryTaskQueue<SystemClock> {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryTaskQueue<SystemClock> {
    /// Creates an empty queue using the system clock.
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }
}

impl<C: QueueClock> InMemoryTaskQueue<C> {
    /// Creates an empty queue with an injected clock.
    #[must_use]
    pub fn with_clock(clock: Arc<C>) -> Self {
        Self {
            clock,
            inner: Arc::new(Mutex::new(QueueStore::default())),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, QueueStore>, QueueError> {
        self.inner
            .lock()
            .map_err(|_| QueueError::Unavailable("task queue lock is poisoned"))
    }

    fn deadline(now: SystemTime, duration: HumanDuration) -> Result<SystemTime, QueueError> {
        now.checked_add(duration.get())
            .ok_or(QueueError::TimeOverflow)
    }

    fn recover_expired(store: &mut QueueStore, now: SystemTime) -> Result<(), QueueError> {
        let expired: Vec<_> = store
            .entries
            .iter()
            .filter_map(|(task_id, entry)| match entry.state {
                EntryState::Leased {
                    lease_id, deadline, ..
                } if deadline <= now => Some((*task_id, lease_id)),
                _ => None,
            })
            .collect();
        for (task_id, lease_id) in expired {
            let sequence = store.allocate_sequence()?;
            store.leases.remove(&lease_id);
            if let Some(entry) = store.entries.get_mut(&task_id) {
                entry.sequence = sequence;
                entry.state = EntryState::Available { at: now };
            }
        }
        Ok(())
    }

    fn validate_owner(
        entry: &QueueEntry,
        lease_id: LeaseId,
        consumer: NodeId,
    ) -> Result<(), QueueError> {
        match entry.state {
            EntryState::Leased {
                consumer: owner, ..
            } if owner == consumer => Ok(()),
            EntryState::Leased {
                consumer: owner, ..
            } => Err(QueueError::LeaseOwnerMismatch {
                lease_id,
                expected: owner,
                actual: consumer,
            }),
            EntryState::Available { .. } => Err(QueueError::LeaseNotFound(lease_id)),
        }
    }
}

impl<C: QueueClock> TaskQueue for InMemoryTaskQueue<C> {
    fn enqueue<'a>(&'a self, request: EnqueueRequest) -> QueueFuture<'a, ()> {
        Box::pin(async move {
            let now = self.clock.now()?;
            let available_at = match request.delay() {
                Some(delay) => Self::deadline(now, delay)?,
                None => now,
            };
            let mut store = self.lock()?;
            if store.entries.contains_key(&request.task_id()) {
                return Err(QueueError::AlreadyQueued(request.task_id()));
            }
            let sequence = store.allocate_sequence()?;
            store.entries.insert(
                request.task_id(),
                QueueEntry {
                    priority: request.priority(),
                    sequence,
                    delivery_attempts: 0,
                    state: EntryState::Available { at: available_at },
                },
            );
            Ok(())
        })
    }

    fn lease<'a>(
        &'a self,
        consumer: NodeId,
        duration: HumanDuration,
    ) -> QueueFuture<'a, Option<TaskLease>> {
        Box::pin(async move {
            let now = self.clock.now()?;
            let deadline = Self::deadline(now, duration)?;
            let mut store = self.lock()?;
            Self::recover_expired(&mut store, now)?;

            let task_id = store
                .entries
                .iter()
                .filter(
                    |(_, entry)| matches!(entry.state, EntryState::Available { at } if at <= now),
                )
                .max_by_key(|(_, entry)| (entry.priority.weight(), Reverse(entry.sequence)))
                .map(|(task_id, _)| *task_id);
            let Some(task_id) = task_id else {
                return Ok(None);
            };

            let lease_id = LeaseId::new();
            let entry = store
                .entries
                .get_mut(&task_id)
                .ok_or(QueueError::Unavailable(
                    "selected task disappeared while queue was locked",
                ))?;
            entry.delivery_attempts = entry
                .delivery_attempts
                .checked_add(1)
                .ok_or(QueueError::DeliveryAttemptsExhausted(task_id))?;
            entry.state = EntryState::Leased {
                lease_id,
                consumer,
                deadline,
            };
            let lease = TaskLease::new(
                lease_id,
                task_id,
                consumer,
                entry.priority,
                entry.delivery_attempts,
                deadline,
            );
            store.leases.insert(lease_id, task_id);
            Ok(Some(lease))
        })
    }

    fn acknowledge<'a>(&'a self, lease_id: LeaseId, consumer: NodeId) -> QueueFuture<'a, TaskId> {
        Box::pin(async move {
            let now = self.clock.now()?;
            let mut store = self.lock()?;
            Self::recover_expired(&mut store, now)?;
            let task_id = *store
                .leases
                .get(&lease_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            let entry = store
                .entries
                .get(&task_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            Self::validate_owner(entry, lease_id, consumer)?;
            store.leases.remove(&lease_id);
            store.entries.remove(&task_id);
            Ok(task_id)
        })
    }

    fn release<'a>(
        &'a self,
        lease_id: LeaseId,
        consumer: NodeId,
        delay: Option<HumanDuration>,
    ) -> QueueFuture<'a, TaskId> {
        Box::pin(async move {
            let now = self.clock.now()?;
            let available_at = match delay {
                Some(delay) => Self::deadline(now, delay)?,
                None => now,
            };
            let mut store = self.lock()?;
            Self::recover_expired(&mut store, now)?;
            let task_id = *store
                .leases
                .get(&lease_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            let entry = store
                .entries
                .get(&task_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            Self::validate_owner(entry, lease_id, consumer)?;
            let sequence = store.allocate_sequence()?;
            let entry = store
                .entries
                .get_mut(&task_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            entry.sequence = sequence;
            entry.state = EntryState::Available { at: available_at };
            store.leases.remove(&lease_id);
            Ok(task_id)
        })
    }

    fn extend<'a>(
        &'a self,
        lease_id: LeaseId,
        consumer: NodeId,
        duration: HumanDuration,
    ) -> QueueFuture<'a, TaskLease> {
        Box::pin(async move {
            let now = self.clock.now()?;
            let deadline = Self::deadline(now, duration)?;
            let mut store = self.lock()?;
            Self::recover_expired(&mut store, now)?;
            let task_id = *store
                .leases
                .get(&lease_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            let entry = store
                .entries
                .get_mut(&task_id)
                .ok_or(QueueError::LeaseNotFound(lease_id))?;
            Self::validate_owner(entry, lease_id, consumer)?;
            entry.state = EntryState::Leased {
                lease_id,
                consumer,
                deadline,
            };
            Ok(TaskLease::new(
                lease_id,
                task_id,
                consumer,
                entry.priority,
                entry.delivery_attempts,
                deadline,
            ))
        })
    }

    fn stats<'a>(&'a self) -> QueueFuture<'a, QueueStats> {
        Box::pin(async move {
            let now = self.clock.now()?;
            let mut store = self.lock()?;
            Self::recover_expired(&mut store, now)?;
            let mut ready = 0;
            let mut delayed = 0;
            let mut leased = 0;
            for entry in store.entries.values() {
                match entry.state {
                    EntryState::Available { at } if at <= now => ready += 1,
                    EntryState::Available { .. } => delayed += 1,
                    EntryState::Leased { .. } => leased += 1,
                }
            }
            Ok(QueueStats::new(ready, delayed, leased))
        })
    }
}
