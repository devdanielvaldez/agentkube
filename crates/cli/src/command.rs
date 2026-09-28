//! Command orchestration for every required `akctl` operation.
//!
//! Each command performs real API operations with optimistic-concurrency
//! semantics. Multi-document apply stops on the first failure and reports how
//! many documents were applied without claiming rollback.

use crate::{
    args::{Command, GetResource, OutputMode, ScaleResource, SingleResource},
    client::ApiClient,
    config::ResolvedConfig,
    document::{Manifest, load_documents_from_path},
    error::CliError,
    output,
};
use agentkube_agents::{AgentDefinition, AgentDeployment, ReplicaCount};
use agentkube_core::{Metadata, Namespace};
use agentkube_protocol::{ApiVersion, ListMetadata, ListResponse, ResourceKind, TypeMeta};
use agentkube_tasks::{AgentTask, TaskDocument};
use std::time::Duration;

/// Validates `--page-size` in the inclusive range `1..=200`.
pub fn validate_page_size(raw: Option<u32>) -> Result<Option<u32>, CliError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            if (1..=200).contains(&value) {
                Ok(Some(value))
            } else {
                Err(CliError::usage(format!(
                    "invalid --page-size {value}: expected a value in 1..=200"
                )))
            }
        }
    }
}

/// Executes one parsed CLI command.
pub async fn execute(
    command: Command,
    config: &ResolvedConfig,
    client: &ApiClient,
) -> Result<(), CliError> {
    match command {
        Command::Version => run_version(config),
        Command::Health => run_health(client, config).await,
        Command::Status => run_status(client, config).await,
        Command::Apply { file, dry_run } => run_apply(client, config, &file, dry_run).await,
        Command::Get {
            resource,
            name,
            page_size,
            continue_token,
        } => run_get(client, config, resource, name, page_size, continue_token).await,
        Command::Describe { resource, name } => run_describe(client, config, resource, &name).await,
        Command::Logs { name, follow } => run_logs(client, config, &name, follow).await,
        Command::Delete { resource, name } => run_delete(client, config, resource, &name).await,
        Command::Scale {
            resource,
            name,
            replicas,
        } => run_scale(client, config, resource, &name, replicas).await,
    }
}

async fn run_logs(
    client: &ApiClient,
    config: &ResolvedConfig,
    name: &str,
    follow: bool,
) -> Result<(), CliError> {
    let mut last_revision = None;
    loop {
        let document = client.get_task(name).await?;
        let status = document.status().ok_or_else(|| {
            CliError::transport(
                format!("get logs for task {name:?}"),
                "server returned a task without status",
            )
        })?;

        if follow && last_revision != Some(status.revision()) {
            eprintln!(
                "task {name:?}: state={:?} attempts={} agent={}",
                status.state(),
                status.attempts_started(),
                status
                    .assigned_agent()
                    .map_or_else(|| "-".to_owned(), |agent| agent.to_string())
            );
            last_revision = Some(status.revision());
        }

        if !follow || status.state().is_terminal() {
            return render_task_logs(&document, config.output.unwrap_or(OutputMode::Table));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn render_task_logs(document: &TaskDocument, mode: OutputMode) -> Result<(), CliError> {
    let name = document.metadata().name().as_str();
    let status = document.status().ok_or_else(|| {
        CliError::transport(
            format!("render logs for task {name:?}"),
            "task has no status",
        )
    })?;
    let record = serde_json::json!({
        "task": name,
        "taskId": status.task_id(),
        "state": status.state(),
        "attempts": status.attempts_started(),
        "assignedAgent": status.assigned_agent(),
        "result": status.result(),
        "failure": status.failure(),
    });
    match mode {
        OutputMode::Json => output::print_json(&record),
        OutputMode::Yaml => output::print_yaml(&record),
        OutputMode::Table => {
            if let Some(result) = status.result() {
                output::print_line(result.output())
            } else if let Some(failure) = status.failure() {
                output::print_line(&format!(
                    "task {name:?} failed ({:?}): {}",
                    failure.kind(),
                    failure.message()
                ))
            } else {
                output::print_line(&format!(
                    "task {name:?} is {:?}; no execution output is available yet",
                    status.state()
                ))
            }
        }
    }
}

fn output_for_get(config: &ResolvedConfig) -> OutputMode {
    config.output.unwrap_or(OutputMode::Table)
}

fn output_for_describe(config: &ResolvedConfig) -> OutputMode {
    config.output.unwrap_or(OutputMode::Yaml)
}

fn run_version(config: &ResolvedConfig) -> Result<(), CliError> {
    let version = env!("CARGO_PKG_VERSION");
    match config.output.unwrap_or(OutputMode::Table) {
        OutputMode::Table => output::print_line(&format!("akctl v{version}")),
        OutputMode::Json => output::print_json(&serde_json::json!({
            "clientVersion": version,
        })),
        OutputMode::Yaml => output::print_yaml(&serde_json::json!({
            "clientVersion": version,
        })),
    }
}

async fn run_health(client: &ApiClient, config: &ResolvedConfig) -> Result<(), CliError> {
    let health = client.health().await?;
    let ready = client.ready().await?;
    match output_for_get(config) {
        OutputMode::Table => output::print_health_table(&health, &ready),
        OutputMode::Json => output::print_json(&serde_json::json!({
            "health": health,
            "ready": ready,
        })),
        OutputMode::Yaml => output::print_yaml(&serde_json::json!({
            "health": health,
            "ready": ready,
        })),
    }
}

async fn run_status(client: &ApiClient, config: &ResolvedConfig) -> Result<(), CliError> {
    let health = client.health().await?;
    let ready = client.ready().await?;
    let agents = client.list_all_agents(None, None).await?;
    let deployments = client.list_all_deployments(None, None).await?;
    let tasks = client.list_all_tasks(None, None).await?;
    match output_for_get(config) {
        OutputMode::Table => output::print_status_table(
            client.base_url(),
            &health,
            &ready,
            &agents,
            &deployments,
            &tasks,
            config.no_color,
        ),
        OutputMode::Json => output::print_json(&serde_json::json!({
            "server": {
                "url": client.base_url(),
                "health": health,
                "ready": ready,
            },
            "summary": output::summarize_status(&agents, &deployments, &tasks),
            "agents": agents,
            "deployments": deployments,
            "tasks": tasks,
        })),
        OutputMode::Yaml => output::print_yaml(&serde_json::json!({
            "server": {
                "url": client.base_url(),
                "health": health,
                "ready": ready,
            },
            "summary": output::summarize_status(&agents, &deployments, &tasks),
            "agents": agents,
            "deployments": deployments,
            "tasks": tasks,
        })),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_get(
    client: &ApiClient,
    config: &ResolvedConfig,
    resource: GetResource,
    name: Option<String>,
    page_size: Option<u32>,
    continue_token: Option<String>,
) -> Result<(), CliError> {
    let page_size = validate_page_size(page_size)?;
    let mode = output_for_get(config);
    if resource == GetResource::Nodes {
        if page_size.is_some() || continue_token.is_some() {
            return Err(CliError::usage(
                "nodes are listed without pagination: omit --page-size and --continue",
            ));
        }
        if name.is_some() {
            return Err(CliError::usage(
                "nodes do not support direct lookup: omit NAME to list",
            ));
        }
        let items = client.list_nodes().await?;
        return render_node_list(&items, mode, config.no_color);
    }
    match (resource, name) {
        (GetResource::Agents, Some(value)) => {
            let document = client.get_agent(&value).await?;
            render_single_agent(&document, mode, config.no_color)
        }
        (GetResource::Agents, None) => {
            let items = client
                .list_all_agents(page_size, continue_token.as_deref())
                .await?;
            render_agent_list(&items, mode, config.no_color)
        }
        (GetResource::Deployments, Some(value)) => {
            let document = client.get_deployment(&value).await?;
            render_single_deployment(&document, mode, config.no_color)
        }
        (GetResource::Deployments, None) => {
            let items = client
                .list_all_deployments(page_size, continue_token.as_deref())
                .await?;
            render_deployment_list(&items, mode, config.no_color)
        }
        (GetResource::Tasks, Some(value)) => {
            let document = client.get_task(&value).await?;
            render_single_task(&document, mode, config.no_color)
        }
        (GetResource::Tasks, None) => {
            let items = client
                .list_all_tasks(page_size, continue_token.as_deref())
                .await?;
            render_task_list(&items, mode, config.no_color)
        }
        (GetResource::Nodes, _) => {
            unreachable!("nodes return before resource dispatch")
        }
    }
}

async fn run_describe(
    client: &ApiClient,
    config: &ResolvedConfig,
    resource: SingleResource,
    name: &str,
) -> Result<(), CliError> {
    let mode = output_for_describe(config);
    match resource {
        SingleResource::Agent => {
            let document = client.get_agent(name).await?;
            render_single_agent(&document, mode, config.no_color)
        }
        SingleResource::Deployment => {
            let document = client.get_deployment(name).await?;
            render_single_deployment(&document, mode, config.no_color)
        }
        SingleResource::Task => {
            let document = client.get_task(name).await?;
            render_single_task(&document, mode, config.no_color)
        }
    }
}

async fn run_delete(
    client: &ApiClient,
    config: &ResolvedConfig,
    resource: SingleResource,
    name: &str,
) -> Result<(), CliError> {
    let mode = output_for_get(config);
    match resource {
        SingleResource::Agent => {
            client.delete_agent(name).await?;
            render_deletion("agent", name, mode)
        }
        SingleResource::Deployment => {
            client.delete_deployment(name).await?;
            render_deletion("deployment", name, mode)
        }
        SingleResource::Task => {
            client.delete_task(name).await?;
            render_deletion("task", name, mode)
        }
    }
}

fn render_deletion(kind: &str, name: &str, mode: OutputMode) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_line(&format!("{kind} {name:?} deleted")),
        OutputMode::Json => output::print_json(&serde_json::json!({
            "kind": kind,
            "name": name,
            "status": "deleted",
        })),
        OutputMode::Yaml => output::print_yaml(&serde_json::json!({
            "kind": kind,
            "name": name,
            "status": "deleted",
        })),
    }
}

async fn run_scale(
    client: &ApiClient,
    config: &ResolvedConfig,
    resource: ScaleResource,
    name: &str,
    replicas: u32,
) -> Result<(), CliError> {
    let ScaleResource::Deployment = resource;
    let desired = ReplicaCount::new(replicas)
        .map_err(|error| CliError::usage(format!("invalid --replicas {replicas}: {error}")))?;
    let operation = format!("scale deployment {name:?}");
    let current = client.get_deployment(name).await?;
    let mut deployment = AgentDeployment::from_document(current).map_err(|error| {
        CliError::transport(
            &operation,
            format!("server returned an invalid deployment: {error}"),
        )
    })?;
    deployment.spec_mut().scale(desired);
    let desired_document = deployment.into_document();
    let updated = client.replace_deployment(name, &desired_document).await?;
    let mode = output_for_get(config);
    match mode {
        OutputMode::Table => {
            output::print_line(&format!("deployment {name:?} scaled to {replicas}"))
        }
        OutputMode::Json => output::print_json(&updated),
        OutputMode::Yaml => output::print_yaml(&updated),
    }
}

async fn run_apply(
    client: &ApiClient,
    config: &ResolvedConfig,
    file: &str,
    dry_run: Option<String>,
) -> Result<(), CliError> {
    if let Some(mode) = dry_run {
        if mode != "client" {
            return Err(CliError::usage(format!(
                "invalid --dry-run {mode:?}: only \"client\" is supported"
            )));
        }
        return run_dry_run(file, config);
    }
    let manifests = load_documents_from_path(file)?;
    let total = manifests.len();
    let mode = output_for_get(config);
    // For machine-readable apply, collect results and render once so stdout
    // contains only valid JSON/YAML.
    let mut machine_results: Vec<serde_json::Value> = Vec::new();
    let mut applied: usize = 0;
    for manifest in &manifests {
        match apply_single(client, manifest).await {
            Ok((message, persisted)) => {
                applied += 1;
                match mode {
                    OutputMode::Table => {
                        output::print_line(&message)?;
                    }
                    OutputMode::Json | OutputMode::Yaml => {
                        machine_results.push(persisted);
                    }
                }
            }
            Err(error) => {
                if total > 1 {
                    let message = error.to_string();
                    if applied > 0 {
                        eprintln!(
                            "{applied} of {total} documents were applied before the failure; no rollback was performed"
                        );
                    }
                    return Err(CliError::partial(applied, total, message));
                }
                return Err(error);
            }
        }
    }
    match mode {
        OutputMode::Table => Ok(()),
        OutputMode::Json => {
            if machine_results.len() == 1 {
                output::print_json(&machine_results[0])
            } else {
                output::print_json(&machine_results)
            }
        }
        OutputMode::Yaml => {
            if machine_results.len() == 1 {
                output::print_yaml(&machine_results[0])
            } else {
                output::print_yaml(&machine_results)
            }
        }
    }
}

fn run_dry_run(file: &str, config: &ResolvedConfig) -> Result<(), CliError> {
    let manifests = load_documents_from_path(file)?;
    // Construction below re-validates domain invariants without requests.
    for manifest in &manifests {
        validate_manifest_locally(manifest)?;
    }
    let mode = config.output.unwrap_or(OutputMode::Table);
    match mode {
        OutputMode::Table => {
            for manifest in &manifests {
                output::print_line(&format!(
                    "{} {:?} validated (dry-run: no requests sent)",
                    manifest.kind(),
                    manifest.name()
                ))?;
            }
            Ok(())
        }
        OutputMode::Json => {
            let values: Vec<serde_json::Value> = manifests
                .iter()
                .map(|manifest| {
                    serde_json::json!({
                        "kind": manifest.kind(),
                        "name": manifest.name(),
                        "namespace": manifest.namespace(),
                        "dryRun": "client",
                    })
                })
                .collect();
            output::print_json(&values)
        }
        OutputMode::Yaml => {
            let values: Vec<serde_json::Value> = manifests
                .iter()
                .map(|manifest| {
                    serde_json::json!({
                        "kind": manifest.kind(),
                        "name": manifest.name(),
                        "namespace": manifest.namespace(),
                        "dryRun": "client",
                    })
                })
                .collect();
            output::print_yaml(&values)
        }
    }
}

fn validate_manifest_locally(manifest: &Manifest) -> Result<(), CliError> {
    let namespace = Namespace::new(manifest.namespace())
        .map_err(|error| CliError::invalid_input(format!("invalid namespace: {error}")))?;
    let metadata = Metadata::in_namespace(manifest.name(), namespace)
        .map_err(|error| CliError::invalid_input(format!("invalid metadata: {error}")))?;
    match manifest {
        Manifest::Agent { spec, .. } => {
            let _ = AgentDefinition::new(metadata, spec.clone());
            Ok(())
        }
        Manifest::Deployment { spec, .. } => {
            AgentDeployment::new(metadata, spec.clone()).map_err(|error| {
                CliError::invalid_input(format!("invalid AgentDeployment: {error}"))
            })?;
            Ok(())
        }
        Manifest::Task { spec, .. } => {
            let _ = AgentTask::new(metadata, spec.clone());
            Ok(())
        }
    }
}

/// Applies one manifest, returning a human-readable result line and the
/// persisted document encoded as JSON for machine-readable output.
async fn apply_single(
    client: &ApiClient,
    manifest: &Manifest,
) -> Result<(String, serde_json::Value), CliError> {
    match manifest {
        Manifest::Agent {
            name,
            namespace,
            spec,
        } => apply_agent(client, name, namespace, spec).await,
        Manifest::Deployment {
            name,
            namespace,
            spec,
        } => apply_deployment(client, name, namespace, spec).await,
        Manifest::Task {
            name,
            namespace,
            spec,
        } => apply_task(client, name, namespace, spec).await,
    }
}

fn encode(value: impl serde::Serialize, operation: &str) -> Result<serde_json::Value, CliError> {
    serde_json::to_value(value)
        .map_err(|error| CliError::transport(operation, format!("cannot encode document: {error}")))
}

async fn apply_agent(
    client: &ApiClient,
    name: &str,
    namespace: &str,
    spec: &agentkube_agents::AgentSpec,
) -> Result<(String, serde_json::Value), CliError> {
    let parsed_namespace = Namespace::new(namespace).map_err(|error| {
        CliError::invalid_input(format!("invalid namespace {namespace:?}: {error}"))
    })?;
    match client.get_agent(name).await {
        Ok(current) => {
            let server_metadata = current.metadata().clone();
            let desired = AgentDefinition::new(server_metadata, spec.clone());
            let document = desired.into_document();
            let persisted = client.replace_agent(name, &document).await?;
            let message = format!("agent {name:?} configured");
            let value = encode(persisted, "apply agent")?;
            Ok((message, value))
        }
        Err(error) => {
            if is_not_found(&error) {
                let metadata = Metadata::in_namespace(name, parsed_namespace).map_err(|error| {
                    CliError::invalid_input(format!("invalid metadata: {error}"))
                })?;
                let desired = AgentDefinition::new(metadata, spec.clone());
                let persisted = client.create_agent(&desired.into_document()).await?;
                let message = format!("agent {name:?} created");
                let value = encode(persisted, "apply agent")?;
                Ok((message, value))
            } else {
                Err(error)
            }
        }
    }
}

async fn apply_deployment(
    client: &ApiClient,
    name: &str,
    namespace: &str,
    spec: &agentkube_agents::AgentDeploymentSpec,
) -> Result<(String, serde_json::Value), CliError> {
    let parsed_namespace = Namespace::new(namespace).map_err(|error| {
        CliError::invalid_input(format!("invalid namespace {namespace:?}: {error}"))
    })?;
    match client.get_deployment(name).await {
        Ok(current) => {
            let server_metadata = current.metadata().clone();
            let desired = AgentDeployment::new(server_metadata, spec.clone()).map_err(|error| {
                CliError::invalid_input(format!("invalid AgentDeployment: {error}"))
            })?;
            let persisted = client
                .replace_deployment(name, &desired.into_document())
                .await?;
            let message = format!("deployment {name:?} configured");
            let value = encode(persisted, "apply deployment")?;
            Ok((message, value))
        }
        Err(error) => {
            if is_not_found(&error) {
                let metadata = Metadata::in_namespace(name, parsed_namespace).map_err(|error| {
                    CliError::invalid_input(format!("invalid metadata: {error}"))
                })?;
                let desired = AgentDeployment::new(metadata, spec.clone()).map_err(|error| {
                    CliError::invalid_input(format!("invalid AgentDeployment: {error}"))
                })?;
                let persisted = client.create_deployment(&desired.into_document()).await?;
                let message = format!("deployment {name:?} created");
                let value = encode(persisted, "apply deployment")?;
                Ok((message, value))
            } else {
                Err(error)
            }
        }
    }
}

async fn apply_task(
    client: &ApiClient,
    name: &str,
    namespace: &str,
    spec: &agentkube_tasks::TaskSpec,
) -> Result<(String, serde_json::Value), CliError> {
    let parsed_namespace = Namespace::new(namespace).map_err(|error| {
        CliError::invalid_input(format!("invalid namespace {namespace:?}: {error}"))
    })?;
    let metadata = Metadata::in_namespace(name, parsed_namespace)
        .map_err(|error| CliError::invalid_input(format!("invalid metadata: {error}")))?;
    // Tasks are create-only execution records; never replace.
    let task = AgentTask::new(metadata, spec.clone());
    let persisted = client.create_task(&task.into_document()).await?;
    let message = format!("task {name:?} created");
    let value = encode(persisted, "apply task")?;
    Ok((message, value))
}

fn is_not_found(error: &CliError) -> bool {
    match error {
        CliError::Api { status, .. } => *status == 404,
        _ => false,
    }
}

fn render_single_agent(
    document: &agentkube_agents::AgentDocument,
    mode: OutputMode,
    no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_agents_table(std::slice::from_ref(document), no_color),
        OutputMode::Json => output::print_json(document),
        OutputMode::Yaml => output::print_yaml(document),
    }
}

fn render_agent_list(
    items: &[agentkube_agents::AgentDocument],
    mode: OutputMode,
    no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_agents_table(items, no_color),
        OutputMode::Json => output::print_json(&agent_list_response(items)),
        OutputMode::Yaml => output::print_yaml(&agent_list_response(items)),
    }
}

fn render_single_deployment(
    document: &agentkube_agents::AgentDeploymentDocument,
    mode: OutputMode,
    no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => {
            output::print_deployments_table(std::slice::from_ref(document), no_color)
        }
        OutputMode::Json => output::print_json(document),
        OutputMode::Yaml => output::print_yaml(document),
    }
}

fn render_deployment_list(
    items: &[agentkube_agents::AgentDeploymentDocument],
    mode: OutputMode,
    no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_deployments_table(items, no_color),
        OutputMode::Json => output::print_json(&deployment_list_response(items)),
        OutputMode::Yaml => output::print_yaml(&deployment_list_response(items)),
    }
}

fn render_single_task(
    document: &agentkube_tasks::TaskDocument,
    mode: OutputMode,
    no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_tasks_table(std::slice::from_ref(document), no_color),
        OutputMode::Json => output::print_json(document),
        OutputMode::Yaml => output::print_yaml(document),
    }
}

fn render_task_list(
    items: &[agentkube_tasks::TaskDocument],
    mode: OutputMode,
    no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_tasks_table(items, no_color),
        OutputMode::Json => output::print_json(&task_list_response(items)),
        OutputMode::Yaml => output::print_yaml(&task_list_response(items)),
    }
}

fn render_node_list(
    items: &[crate::client::NodeStatus],
    mode: OutputMode,
    _no_color: bool,
) -> Result<(), CliError> {
    match mode {
        OutputMode::Table => output::print_nodes_table(items),
        OutputMode::Json => output::print_json(&items),
        OutputMode::Yaml => output::print_yaml(&items),
    }
}

fn list_type(kind: &str) -> TypeMeta {
    TypeMeta::new(
        ApiVersion::new("agentkube.ai/v1").expect("static API version is valid"),
        ResourceKind::new(kind).expect("static list kind is valid"),
    )
}

fn agent_list_response(
    items: &[agentkube_agents::AgentDocument],
) -> ListResponse<agentkube_agents::AgentDocument> {
    ListResponse::new(list_type("AgentList"), ListMetadata::new(), items.to_vec())
}

fn deployment_list_response(
    items: &[agentkube_agents::AgentDeploymentDocument],
) -> ListResponse<agentkube_agents::AgentDeploymentDocument> {
    ListResponse::new(
        list_type("AgentDeploymentList"),
        ListMetadata::new(),
        items.to_vec(),
    )
}

fn task_list_response(
    items: &[agentkube_tasks::TaskDocument],
) -> ListResponse<agentkube_tasks::TaskDocument> {
    ListResponse::new(
        list_type("AgentTaskList"),
        ListMetadata::new(),
        items.to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_validation_enforces_bounds() {
        assert!(validate_page_size(None).unwrap().is_none());
        assert_eq!(validate_page_size(Some(1)).unwrap(), Some(1));
        assert_eq!(validate_page_size(Some(200)).unwrap(), Some(200));
        assert!(validate_page_size(Some(0)).is_err());
        assert!(validate_page_size(Some(201)).is_err());
    }
}
