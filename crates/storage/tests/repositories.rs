use agentkube_agents::{AgentInstance, AgentInstanceState};
use agentkube_core::{Metadata, Namespace, Resource, ResourceUid, ResourceVersion};
use agentkube_storage::{
    EntityRepository, EntityRepositoryError, InMemoryEntityRepository, InMemoryResourceRepository,
    ResourceKey, ResourceRepository, StorageError,
};
use agentkube_tasks::{AgentTask, Objective, TaskSpec, TaskState};
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
        Poll::Pending => panic!("in-memory repository unexpectedly returned a pending future"),
    }
}

fn task(name: &str, namespace: &str) -> AgentTask {
    AgentTask::new(
        Metadata::in_namespace(name, Namespace::new(namespace).unwrap()).unwrap(),
        TaskSpec::new(Objective::new(format!("Complete {name}")).unwrap()),
    )
}

#[test]
fn resource_repository_supports_the_full_crud_contract() {
    let repository = InMemoryResourceRepository::new();
    let original = task("compile", "build");
    let key = ResourceKey::from(original.metadata());
    let uid = original.metadata().uid();

    let created = ready(repository.create(original)).unwrap();
    assert_eq!(created.metadata().resource_version().get(), 1);
    assert_eq!(ready(repository.get(&key)).unwrap(), Some(created.clone()));
    assert_eq!(
        ready(repository.get_by_uid(uid)).unwrap(),
        Some(created.clone())
    );

    let mut changed = created;
    changed.enqueue().unwrap();
    let replaced = ready(repository.replace(changed)).unwrap();
    assert_eq!(replaced.status().state(), TaskState::Queued);
    assert_eq!(replaced.metadata().resource_version().get(), 2);

    let removed = ready(repository.delete(&key, ResourceVersion::new(2).unwrap())).unwrap();
    assert_eq!(removed, replaced);
    assert_eq!(ready(repository.get(&key)).unwrap(), None);
    assert_eq!(ready(repository.get_by_uid(uid)).unwrap(), None);
}

#[test]
fn resource_list_is_key_ordered_and_can_be_scoped_to_a_namespace() {
    let repository = InMemoryResourceRepository::new();
    for resource in [
        task("zeta", "production"),
        task("beta", "development"),
        task("alpha", "development"),
    ] {
        ready(repository.create(resource)).unwrap();
    }

    let all = ready(repository.list(None)).unwrap();
    let all_keys: Vec<_> = all
        .iter()
        .map(|resource| ResourceKey::from(resource.metadata()).to_string())
        .collect();
    assert_eq!(
        all_keys,
        ["development/alpha", "development/beta", "production/zeta"]
    );

    let development = ready(repository.list(Some(Namespace::new("development").unwrap()))).unwrap();
    assert_eq!(development.len(), 2);
    assert!(
        development
            .iter()
            .all(|resource| resource.metadata().namespace().as_str() == "development")
    );
}

#[test]
fn resource_repository_rejects_duplicates_stale_writes_and_identity_changes() {
    let repository = InMemoryResourceRepository::new();
    let original = task("review", "default");
    let stale = original.clone();
    ready(repository.create(original.clone())).unwrap();

    assert!(matches!(
        ready(repository.create(original)),
        Err(StorageError::AlreadyExists(_))
    ));

    let mut current = stale.clone();
    current.enqueue().unwrap();
    ready(repository.replace(current)).unwrap();
    assert!(matches!(
        ready(repository.replace(stale)),
        Err(StorageError::Conflict { .. })
    ));

    let different_identity = task("review", "default");
    assert!(matches!(
        ready(repository.replace(different_identity)),
        Err(StorageError::IdentityChanged { .. })
    ));
}

#[test]
fn resource_compare_and_swap_allows_exactly_one_concurrent_writer() {
    let repository = InMemoryResourceRepository::new();
    let original = task("race", "default");
    ready(repository.create(original.clone())).unwrap();

    let first_repository = repository.clone();
    let second_repository = repository.clone();
    let first = original.clone();
    let second = original;
    let first_write = std::thread::spawn(move || ready(first_repository.replace(first)));
    let second_write = std::thread::spawn(move || ready(second_repository.replace(second)));
    let results = [first_write.join().unwrap(), second_write.join().unwrap()];

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(StorageError::Conflict { .. })))
            .count(),
        1
    );
}

#[test]
fn resource_create_requires_the_initial_version() {
    let repository = InMemoryResourceRepository::new();
    let mut resource = task("invalid-version", "default");
    resource
        .metadata_mut()
        .set_resource_version(ResourceVersion::new(2).unwrap());

    assert!(matches!(
        ready(repository.create(resource)),
        Err(StorageError::InvalidInitialVersion { .. })
    ));
}

#[test]
fn runtime_entity_repository_persists_domain_owned_revisions() {
    let repository = InMemoryEntityRepository::new();
    let original = AgentInstance::new(ResourceUid::new());
    let id = original.id();
    ready(repository.insert(original.clone())).unwrap();

    let mut scheduled = original;
    scheduled
        .transition(AgentInstanceState::Scheduling)
        .unwrap();
    let stored = ready(repository.replace(scheduled, 1)).unwrap();
    assert_eq!(stored.revision(), 2);
    assert_eq!(stored.state(), AgentInstanceState::Scheduling);
    assert_eq!(ready(repository.get(id)).unwrap(), Some(stored.clone()));

    let removed = ready(repository.delete(id, 2)).unwrap();
    assert_eq!(removed, stored);
    assert_eq!(ready(repository.get(id)).unwrap(), None);
}

#[test]
fn runtime_entity_repository_enforces_revision_cas() {
    let repository = InMemoryEntityRepository::new();
    let original = AgentInstance::new(ResourceUid::new());
    ready(repository.insert(original.clone())).unwrap();

    let mut first = original.clone();
    first.transition(AgentInstanceState::Scheduling).unwrap();
    ready(repository.replace(first, 1)).unwrap();

    let mut stale = original;
    stale.transition(AgentInstanceState::Scheduling).unwrap();
    assert!(matches!(
        ready(repository.replace(stale, 1)),
        Err(EntityRepositoryError::Conflict {
            expected: 1,
            actual: 2,
            ..
        })
    ));
}
