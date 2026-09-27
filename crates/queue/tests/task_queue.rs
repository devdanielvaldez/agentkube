use agentkube_core::{HumanDuration, Metadata, NodeId};
use agentkube_queue::{
    EnqueueRequest, EnqueueRequestError, InMemoryTaskQueue, ManualClock, QueueError, TaskQueue,
};
use agentkube_tasks::{AgentTask, Objective, TaskPriority, TaskSpec};
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("in-memory queue unexpectedly returned a pending future"),
    }
}

fn queued_task(name: &str, priority: TaskPriority) -> AgentTask {
    let mut task = AgentTask::new(
        Metadata::new(name).unwrap(),
        TaskSpec::new(Objective::new(format!("Execute {name}")).unwrap()).with_priority(priority),
    );
    task.enqueue().unwrap();
    task
}

fn duration(value: &str) -> HumanDuration {
    value.parse().unwrap()
}

#[test]
fn enqueue_request_requires_the_queued_domain_state() {
    let pending = AgentTask::new(
        Metadata::new("pending").unwrap(),
        TaskSpec::new(Objective::new("Not ready").unwrap()),
    );

    assert!(matches!(
        EnqueueRequest::from_task(&pending),
        Err(EnqueueRequestError::TaskNotQueued(_))
    ));
}

#[test]
fn queue_dispatches_higher_priorities_first_and_preserves_fifo_order() {
    let queue = InMemoryTaskQueue::new();
    let low = queued_task("low", TaskPriority::Low);
    let first_high = queued_task("first-high", TaskPriority::High);
    let second_high = queued_task("second-high", TaskPriority::High);
    for task in [&low, &first_high, &second_high] {
        ready(queue.enqueue(EnqueueRequest::from_task(task).unwrap())).unwrap();
    }
    let consumer = NodeId::new();

    for expected in [
        first_high.status().task_id(),
        second_high.status().task_id(),
        low.status().task_id(),
    ] {
        let lease = ready(queue.lease(consumer, duration("30s")))
            .unwrap()
            .unwrap();
        assert_eq!(lease.task_id(), expected);
        ready(queue.acknowledge(lease.lease_id(), consumer)).unwrap();
    }
    assert!(
        ready(queue.lease(consumer, duration("30s")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn delayed_tasks_become_visible_only_after_their_deadline() {
    let clock = Arc::new(ManualClock::new());
    let queue = InMemoryTaskQueue::with_clock(Arc::clone(&clock));
    let task = queued_task("delayed", TaskPriority::Normal);
    let request = EnqueueRequest::from_task(&task)
        .unwrap()
        .with_delay(duration("10s"));
    ready(queue.enqueue(request)).unwrap();

    let stats = ready(queue.stats()).unwrap();
    assert_eq!((stats.ready(), stats.delayed(), stats.leased()), (0, 1, 0));
    assert!(
        ready(queue.lease(NodeId::new(), duration("5s")))
            .unwrap()
            .is_none()
    );

    clock.advance(duration("10s")).unwrap();
    assert!(
        ready(queue.lease(NodeId::new(), duration("5s")))
            .unwrap()
            .is_some()
    );
}

#[test]
fn expired_leases_are_recovered_with_a_new_delivery_attempt() {
    let clock = Arc::new(ManualClock::new());
    let queue = InMemoryTaskQueue::with_clock(Arc::clone(&clock));
    let task = queued_task("recover", TaskPriority::Normal);
    ready(queue.enqueue(EnqueueRequest::from_task(&task).unwrap())).unwrap();
    let first_consumer = NodeId::new();
    let second_consumer = NodeId::new();
    let first = ready(queue.lease(first_consumer, duration("5s")))
        .unwrap()
        .unwrap();
    assert_eq!(first.delivery_attempt(), 1);

    clock.advance(duration("5s")).unwrap();
    let second = ready(queue.lease(second_consumer, duration("5s")))
        .unwrap()
        .unwrap();
    assert_eq!(second.task_id(), first.task_id());
    assert_eq!(second.delivery_attempt(), 2);
    assert_ne!(second.lease_id(), first.lease_id());
    assert!(matches!(
        ready(queue.acknowledge(first.lease_id(), first_consumer)),
        Err(QueueError::LeaseNotFound(_))
    ));
}

#[test]
fn leases_enforce_consumer_ownership_and_can_be_extended() {
    let clock = Arc::new(ManualClock::new());
    let queue = InMemoryTaskQueue::with_clock(Arc::clone(&clock));
    let task = queued_task("long-running", TaskPriority::Normal);
    ready(queue.enqueue(EnqueueRequest::from_task(&task).unwrap())).unwrap();
    let owner = NodeId::new();
    let intruder = NodeId::new();
    let lease = ready(queue.lease(owner, duration("5s"))).unwrap().unwrap();

    assert!(matches!(
        ready(queue.extend(lease.lease_id(), intruder, duration("10s"))),
        Err(QueueError::LeaseOwnerMismatch { .. })
    ));
    clock.advance(duration("4s")).unwrap();
    let extended = ready(queue.extend(lease.lease_id(), owner, duration("5s"))).unwrap();
    assert!(extended.deadline() > lease.deadline());
    clock.advance(duration("4s")).unwrap();
    assert!(
        ready(queue.lease(intruder, duration("5s")))
            .unwrap()
            .is_none()
    );
    ready(queue.acknowledge(lease.lease_id(), owner)).unwrap();
    assert_eq!(ready(queue.stats()).unwrap().total(), 0);
}

#[test]
fn release_can_apply_retry_backoff_without_changing_fifo_identity() {
    let clock = Arc::new(ManualClock::new());
    let queue = InMemoryTaskQueue::with_clock(Arc::clone(&clock));
    let task = queued_task("retry", TaskPriority::Normal);
    ready(queue.enqueue(EnqueueRequest::from_task(&task).unwrap())).unwrap();
    let consumer = NodeId::new();
    let first = ready(queue.lease(consumer, duration("10s")))
        .unwrap()
        .unwrap();

    ready(queue.release(first.lease_id(), consumer, Some(duration("30s")))).unwrap();
    assert_eq!(ready(queue.stats()).unwrap().delayed(), 1);
    clock.advance(duration("30s")).unwrap();
    let second = ready(queue.lease(consumer, duration("10s")))
        .unwrap()
        .unwrap();
    assert_eq!(second.task_id(), first.task_id());
    assert_eq!(second.delivery_attempt(), 2);
}

#[test]
fn released_work_moves_to_the_back_of_its_priority_level() {
    let queue = InMemoryTaskQueue::new();
    let first_task = queued_task("first", TaskPriority::Normal);
    let second_task = queued_task("second", TaskPriority::Normal);
    ready(queue.enqueue(EnqueueRequest::from_task(&first_task).unwrap())).unwrap();
    ready(queue.enqueue(EnqueueRequest::from_task(&second_task).unwrap())).unwrap();
    let consumer = NodeId::new();

    let first_lease = ready(queue.lease(consumer, duration("10s")))
        .unwrap()
        .unwrap();
    assert_eq!(first_lease.task_id(), first_task.status().task_id());
    ready(queue.release(first_lease.lease_id(), consumer, None)).unwrap();

    let next_lease = ready(queue.lease(consumer, duration("10s")))
        .unwrap()
        .unwrap();
    assert_eq!(next_lease.task_id(), second_task.status().task_id());
}

#[test]
fn duplicate_active_entries_are_rejected() {
    let queue = InMemoryTaskQueue::new();
    let task = queued_task("unique", TaskPriority::Normal);
    let request = EnqueueRequest::from_task(&task).unwrap();
    ready(queue.enqueue(request)).unwrap();

    assert_eq!(
        ready(queue.enqueue(request)),
        Err(QueueError::AlreadyQueued(task.status().task_id()))
    );
}

#[test]
fn concurrent_consumers_cannot_lease_the_same_delivery() {
    let queue = InMemoryTaskQueue::new();
    let task = queued_task("exclusive", TaskPriority::Critical);
    ready(queue.enqueue(EnqueueRequest::from_task(&task).unwrap())).unwrap();

    let workers: Vec<_> = (0..8)
        .map(|_| {
            let queue = queue.clone();
            std::thread::spawn(move || ready(queue.lease(NodeId::new(), duration("30s"))).unwrap())
        })
        .collect();
    let leases: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();

    assert_eq!(leases.iter().filter(|lease| lease.is_some()).count(), 1);
    assert_eq!(ready(queue.stats()).unwrap().leased(), 1);
}
