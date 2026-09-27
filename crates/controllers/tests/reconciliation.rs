use agentkube_agents::{
    AgentConditionType, AgentDefinition, AgentDeployment, AgentDeploymentSpec, AgentInstance,
    AgentInstanceState, AgentPhase, AgentRole, AgentSpec, Capability, ConditionStatus,
    Instructions, ModelName, ModelPolicy, ProviderName, ReplicaCount, RestartPolicy,
};
use agentkube_controllers::{
    AgentDefinitionReconciler, AgentDeploymentReconciler, ControllerError, DeploymentAction,
    ManagedAgentInstance, ReconcileState,
};
use agentkube_core::{Metadata, NodeId, Resource, ResourceUid, TaskId};
use agentkube_storage::{InMemoryResourceRepository, ResourceRepository};
use std::{
    future::Future,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

fn block_on<F: Future>(future: F) -> F::Output {
    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn template(instructions: &str) -> AgentSpec {
    AgentSpec::new(
        AgentRole::new("developer").unwrap(),
        ModelPolicy::fixed(
            ProviderName::new("openai").unwrap(),
            ModelName::new("gpt-5").unwrap(),
        ),
        Instructions::new(instructions).unwrap(),
    )
    .with_capability(Capability::new("coding").unwrap())
}

fn deployment(replicas: u32, restart_policy: RestartPolicy) -> AgentDeployment {
    AgentDeployment::new(
        Metadata::new("coding-agents").unwrap(),
        AgentDeploymentSpec::new(
            ReplicaCount::new(replicas).unwrap(),
            template("Current template"),
        )
        .with_restart_policy(restart_policy),
    )
    .unwrap()
}

fn instance_in_state(definition_uid: ResourceUid, state: AgentInstanceState) -> AgentInstance {
    let mut instance = AgentInstance::new(definition_uid);
    match state {
        AgentInstanceState::Pending => {}
        AgentInstanceState::Scheduling => {
            instance.transition(AgentInstanceState::Scheduling).unwrap();
        }
        AgentInstanceState::Starting => {
            instance.transition(AgentInstanceState::Scheduling).unwrap();
            instance.assign_node(NodeId::new()).unwrap();
            instance.transition(AgentInstanceState::Starting).unwrap();
        }
        AgentInstanceState::Ready => make_ready(&mut instance),
        AgentInstanceState::Running => {
            make_ready(&mut instance);
            instance.begin_task(TaskId::new()).unwrap();
        }
        AgentInstanceState::Paused => {
            make_ready(&mut instance);
            instance.begin_task(TaskId::new()).unwrap();
            instance.transition(AgentInstanceState::Paused).unwrap();
        }
        AgentInstanceState::Completed => {
            make_ready(&mut instance);
            let task_id = TaskId::new();
            instance.begin_task(task_id).unwrap();
            instance.complete_instance(task_id).unwrap();
        }
        AgentInstanceState::Failed => {
            make_ready(&mut instance);
            instance.fail().unwrap();
        }
        AgentInstanceState::Retrying => {
            make_ready(&mut instance);
            instance.fail().unwrap();
            instance.retry().unwrap();
        }
        AgentInstanceState::Terminated => {
            instance.transition(AgentInstanceState::Terminated).unwrap();
        }
    }
    instance
}

fn make_ready(instance: &mut AgentInstance) {
    instance.transition(AgentInstanceState::Scheduling).unwrap();
    instance.assign_node(NodeId::new()).unwrap();
    instance.transition(AgentInstanceState::Starting).unwrap();
    instance.transition(AgentInstanceState::Ready).unwrap();
}

fn managed(
    owner_uid: ResourceUid,
    applied_template: AgentSpec,
    state: AgentInstanceState,
) -> ManagedAgentInstance {
    ManagedAgentInstance::new(
        owner_uid,
        applied_template,
        instance_in_state(ResourceUid::new(), state),
    )
}

#[test]
fn deployment_scales_up_exactly_to_desired_replicas() {
    let deployment = deployment(3, RestartPolicy::OnFailure);

    let plan = AgentDeploymentReconciler::new()
        .reconcile(&deployment, &[])
        .unwrap();

    assert_eq!(plan.state(), ReconcileState::Progressing);
    assert_eq!(plan.actions().len(), 3);
    assert!(plan.actions().iter().all(|action| matches!(
        action,
        DeploymentAction::CreateReplica { owner_uid, template }
            if *owner_uid == deployment.metadata().uid()
                && template == deployment.spec().template()
    )));
    assert!(plan.resource_update().is_none());
}

#[test]
fn scale_down_prefers_terminal_and_failed_replicas_deterministically() {
    let deployment = deployment(1, RestartPolicy::OnFailure);
    let owner = deployment.metadata().uid();
    let current = deployment.spec().template().clone();
    let completed = managed(owner, current.clone(), AgentInstanceState::Completed);
    let failed = managed(owner, current.clone(), AgentInstanceState::Failed);
    let ready = managed(owner, current, AgentInstanceState::Ready);

    let plan = AgentDeploymentReconciler::new()
        .reconcile(
            &deployment,
            &[ready.clone(), failed.clone(), completed.clone()],
        )
        .unwrap();

    let deleted: Vec<_> = plan
        .actions()
        .iter()
        .filter_map(|action| match action {
            DeploymentAction::DeleteReplica { agent_id, .. } => Some(*agent_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        deleted,
        vec![completed.instance().id(), failed.instance().id()]
    );
    assert!(!deleted.contains(&ready.instance().id()));
}

#[test]
fn rolling_update_replaces_safe_replicas_and_waits_for_running_work() {
    let deployment = deployment(2, RestartPolicy::Always);
    let owner = deployment.metadata().uid();
    let old = template("Old template");
    let ready = managed(owner, old.clone(), AgentInstanceState::Ready);
    let running = managed(owner, old, AgentInstanceState::Running);

    let plan = AgentDeploymentReconciler::new()
        .reconcile(&deployment, &[running.clone(), ready.clone()])
        .unwrap();

    assert_eq!(plan.state(), ReconcileState::Progressing);
    assert_eq!(plan.actions().len(), 1);
    assert!(matches!(
        &plan.actions()[0],
        DeploymentAction::ReplaceReplica { agent_id, template, .. }
            if *agent_id == ready.instance().id()
                && template == deployment.spec().template()
    ));
    assert!(!plan.actions().iter().any(|action| matches!(
        action,
        DeploymentAction::ReplaceReplica { agent_id, .. }
            if *agent_id == running.instance().id()
    )));
}

#[test]
fn restart_policy_controls_failed_and_completed_replicas() {
    let on_failure = deployment(1, RestartPolicy::OnFailure);
    let failed = managed(
        on_failure.metadata().uid(),
        on_failure.spec().template().clone(),
        AgentInstanceState::Failed,
    );
    let retry_plan = AgentDeploymentReconciler::new()
        .reconcile(&on_failure, std::slice::from_ref(&failed))
        .unwrap();
    assert!(matches!(
        retry_plan.actions(),
        [DeploymentAction::RestartReplica { agent_id, .. }]
            if *agent_id == failed.instance().id()
    ));

    let never = deployment(1, RestartPolicy::Never);
    let failed = managed(
        never.metadata().uid(),
        never.spec().template().clone(),
        AgentInstanceState::Failed,
    );
    let blocked = AgentDeploymentReconciler::new()
        .reconcile(&never, &[failed])
        .unwrap();
    assert_eq!(blocked.state(), ReconcileState::Blocked);
    assert!(blocked.actions().is_empty());

    let always = deployment(1, RestartPolicy::Always);
    let completed = managed(
        always.metadata().uid(),
        always.spec().template().clone(),
        AgentInstanceState::Completed,
    );
    let replacement = AgentDeploymentReconciler::new()
        .reconcile(&always, std::slice::from_ref(&completed))
        .unwrap();
    assert!(matches!(
        replacement.actions(),
        [DeploymentAction::ReplaceReplica { agent_id, .. }]
            if *agent_id == completed.instance().id()
    ));
}

#[test]
fn terminated_replicas_are_collected_and_do_not_fill_desired_capacity() {
    let deployment = deployment(1, RestartPolicy::OnFailure);
    let terminated = managed(
        deployment.metadata().uid(),
        deployment.spec().template().clone(),
        AgentInstanceState::Terminated,
    );

    let plan = AgentDeploymentReconciler::new()
        .reconcile(&deployment, std::slice::from_ref(&terminated))
        .unwrap();

    assert_eq!(plan.actions().len(), 2);
    assert!(matches!(
        plan.actions()[0],
        DeploymentAction::DeleteReplica { agent_id, .. }
            if agent_id == terminated.instance().id()
    ));
    assert!(matches!(
        plan.actions()[1],
        DeploymentAction::CreateReplica { .. }
    ));
}

#[test]
fn foreign_ownership_is_ignored_and_duplicate_snapshots_are_rejected() {
    let deployment = deployment(1, RestartPolicy::OnFailure);
    let foreign = managed(
        ResourceUid::new(),
        deployment.spec().template().clone(),
        AgentInstanceState::Ready,
    );
    let scale_up = AgentDeploymentReconciler::new()
        .reconcile(&deployment, std::slice::from_ref(&foreign))
        .unwrap();
    assert!(matches!(
        scale_up.actions(),
        [DeploymentAction::CreateReplica { .. }]
    ));

    let error = AgentDeploymentReconciler::new()
        .reconcile(&deployment, &[foreign.clone(), foreign.clone()])
        .unwrap_err();
    assert_eq!(
        error,
        ControllerError::DuplicateAgent(foreign.instance().id())
    );
}

#[test]
fn persisted_deployment_status_converges_to_an_idempotent_noop() {
    let repository = InMemoryResourceRepository::<AgentDeployment>::new();
    let deployment = block_on(repository.create(deployment(1, RestartPolicy::OnFailure))).unwrap();
    let ready = managed(
        deployment.metadata().uid(),
        deployment.spec().template().clone(),
        AgentInstanceState::Ready,
    );
    let first = AgentDeploymentReconciler::new()
        .reconcile(&deployment, std::slice::from_ref(&ready))
        .unwrap();
    let update = first.resource_update().expect("status changed").clone();
    assert_eq!(update.status().observed_version().get(), 2);
    assert_eq!(update.status().ready_replicas().get(), 1);
    assert_eq!(update.status().available_replicas().get(), 1);
    let persisted = block_on(repository.replace(update)).unwrap();

    let second = AgentDeploymentReconciler::new()
        .reconcile(&persisted, &[ready])
        .unwrap();
    assert_eq!(second.state(), ReconcileState::Converged);
    assert!(second.is_noop());
}

#[test]
fn definition_status_tracks_progress_availability_and_failure() {
    let definition = AgentDefinition::new(
        Metadata::new("backend-agent").unwrap(),
        template("Definition template"),
    );
    let pending = instance_in_state(definition.metadata().uid(), AgentInstanceState::Starting);
    let progressing = AgentDefinitionReconciler::new()
        .reconcile(&definition, &[pending])
        .unwrap();
    assert_eq!(progressing.state(), ReconcileState::Progressing);
    let progressing_status = progressing.resource_update().unwrap().status();
    assert_eq!(progressing_status.phase(), AgentPhase::Pending);
    assert_eq!(
        condition(progressing_status, AgentConditionType::Progressing),
        ConditionStatus::True
    );

    let ready = instance_in_state(definition.metadata().uid(), AgentInstanceState::Ready);
    let failed = instance_in_state(definition.metadata().uid(), AgentInstanceState::Failed);
    let degraded = AgentDefinitionReconciler::new()
        .reconcile(&definition, &[ready, failed])
        .unwrap();
    let status = degraded.resource_update().unwrap().status();
    assert_eq!(status.phase(), AgentPhase::Degraded);
    assert_eq!(
        condition(status, AgentConditionType::Available),
        ConditionStatus::True
    );

    let completed = instance_in_state(definition.metadata().uid(), AgentInstanceState::Completed);
    let unavailable = AgentDefinitionReconciler::new()
        .reconcile(&definition, &[completed])
        .unwrap();
    assert_eq!(unavailable.state(), ReconcileState::Blocked);
    assert_eq!(
        unavailable.resource_update().unwrap().status().phase(),
        AgentPhase::Pending
    );
}

#[test]
fn persisted_definition_status_is_idempotent_and_ignores_foreign_instances() {
    let repository = InMemoryResourceRepository::<AgentDefinition>::new();
    let definition = block_on(repository.create(AgentDefinition::new(
        Metadata::new("backend-agent").unwrap(),
        template("Definition template"),
    )))
    .unwrap();
    let ready = instance_in_state(definition.metadata().uid(), AgentInstanceState::Ready);
    let foreign = instance_in_state(ResourceUid::new(), AgentInstanceState::Failed);
    let first = AgentDefinitionReconciler::new()
        .reconcile(&definition, &[foreign.clone(), ready.clone()])
        .unwrap();
    let persisted =
        block_on(repository.replace(first.resource_update().expect("status changed").clone()))
            .unwrap();
    assert_eq!(persisted.status().observed_version().get(), 2);
    assert_eq!(persisted.status().phase(), AgentPhase::Ready);

    let second = AgentDefinitionReconciler::new()
        .reconcile(&persisted, &[ready, foreign])
        .unwrap();
    assert_eq!(second.state(), ReconcileState::Converged);
    assert!(second.is_noop());
}

fn condition(
    status: &agentkube_agents::AgentStatus,
    condition_type: AgentConditionType,
) -> ConditionStatus {
    status
        .conditions()
        .iter()
        .find(|condition| condition.condition_type() == condition_type)
        .expect("condition exists")
        .status()
}
