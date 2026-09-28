//! Crash recovery replayed from durable task state at boot.
//!
//! The queue is ephemeral by design, so a restart loses all deliveries.
//! Recovery replays durable state back into the queue:
//!
//! - `QUEUED` tasks are re-enqueued (their delivery was lost).
//! - `RUNNING` and waiting tasks failed with `WorkerLost`: the worker holding
//!   them is gone. Retry policy decides, with backoff, whether they run again.
//! - `SCHEDULED` tasks are cancelled: they never executed anywhere.
//! - `PENDING` and terminal tasks are untouched.
//!
//! Infrastructure failures abort the boot loudly. Per-task domain edges are
//! logged and skipped so one poisoned task cannot wedge the operator.

use crate::OperatorError;
use agentkube_core::{HumanDuration, Resource};
use agentkube_queue::{EnqueueRequest, TaskQueue};
use agentkube_storage::ResourceRepository;
use agentkube_tasks::{AgentTask, TaskFailure, TaskFailureKind, TaskOperationError, TaskState};
use std::sync::Arc;

/// Counts of recovery transitions applied at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecoverySummary {
    /// Queued tasks whose deliveries were restored.
    pub requeued: usize,
    /// Running tasks newly failed with `WorkerLost`.
    pub failed: usize,
    /// Scheduled-but-never-executed tasks that were cancelled.
    pub cancelled: usize,
    /// Failed tasks the retry policy admitted back to the queue.
    pub retried: usize,
    /// Tasks left untouched after a domain edge (logged).
    pub skipped: usize,
}

/// Replays durable task state into a fresh queue. See the module docs.
pub async fn recover(
    tasks: &Arc<dyn ResourceRepository<AgentTask>>,
    queue: Arc<dyn TaskQueue>,
) -> Result<RecoverySummary, OperatorError> {
    let stored = tasks.list(None).await?;
    let mut summary = RecoverySummary::default();
    for mut task in stored {
        let name = task.metadata().name().as_str().to_owned();
        match task.status().state() {
            TaskState::Queued => {
                reenqueue(&queue, &task, None).await?;
                summary.requeued += 1;
            }
            TaskState::Running
            | TaskState::WaitingTool
            | TaskState::WaitingAgent
            | TaskState::WaitingApproval => {
                mark_lost(tasks, &queue, &mut task, &name, &mut summary).await?;
            }
            TaskState::Scheduled => {
                task.cancel().map_err(|error| {
                    OperatorError::Repository(format!("cannot cancel task {name:?}: {error}"))
                })?;
                tasks.replace(task).await?;
                eprintln!("recovery: cancelled never-executed task {name:?}");
                summary.cancelled += 1;
            }
            TaskState::Pending
            | TaskState::Completed
            | TaskState::Failed
            | TaskState::Cancelled => {}
        }
    }
    eprintln!(
        "recovery: requeued {}, failed {}, cancelled {}, retried {}, skipped {}",
        summary.requeued, summary.failed, summary.cancelled, summary.retried, summary.skipped
    );
    Ok(summary)
}

/// Fails a task orphaned by a dead worker, then applies retry policy.
async fn mark_lost(
    tasks: &Arc<dyn ResourceRepository<AgentTask>>,
    queue: &Arc<dyn TaskQueue>,
    task: &mut AgentTask,
    name: &str,
    summary: &mut RecoverySummary,
) -> Result<(), OperatorError> {
    if task
        .fail(TaskFailure::new(
            TaskFailureKind::WorkerLost,
            "worker stopped during execution",
        ))
        .is_err()
    {
        eprintln!("recovery: skipping task {name:?} with an un-failable state");
        summary.skipped += 1;
        return Ok(());
    }
    let persisted = tasks.replace(task.clone()).await?;
    *task = persisted;
    summary.failed += 1;
    eprintln!("recovery: failed orphaned task {name:?} as WorkerLost");
    let delay = task.retry_delay();
    match task.retry() {
        Ok(()) => {
            tasks.replace(task.clone()).await?;
            reenqueue(queue, task, delay).await?;
            summary.retried += 1;
            summary.requeued += 1;
            eprintln!("recovery: retry policy requeued task {name:?}");
        }
        Err(TaskOperationError::RetryNotAllowed) => {}
        Err(error) => {
            eprintln!("recovery: skipping task {name:?} after retry edge: {error}");
            summary.skipped += 1;
        }
    }
    Ok(())
}

async fn reenqueue(
    queue: &Arc<dyn TaskQueue>,
    task: &AgentTask,
    delay: Option<HumanDuration>,
) -> Result<(), OperatorError> {
    let request = EnqueueRequest::from_task(task).map_err(|error| {
        OperatorError::Repository(format!(
            "cannot re-enqueue task {:?}: {error}",
            task.metadata().name().as_str()
        ))
    })?;
    let request = match delay {
        Some(delay) => request.with_delay(delay),
        None => request,
    };
    queue.enqueue(request).await?;
    Ok(())
}
