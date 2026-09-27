use crate::{
    ApiState, HttpApiError, metrics::MetricsResponse, nodes::NodeInfo, pagination::paginate,
};
use agentkube_agents::{
    AgentDefinition, AgentDeployment, AgentDeploymentDocument, AgentDocument, AgentPhase,
};
use agentkube_core::{Metadata, Namespace, Resource, ResourceName};
use agentkube_protocol::{
    ApiVersion, ListOptions, ListResponse, ResourceDocument, ResourceKind, TypeMeta,
};
use agentkube_queue::EnqueueRequest;
use agentkube_storage::ResourceKey;
use agentkube_tasks::{AgentTask, TaskDocument, TaskState};
use axum::{
    Json,
    extract::{
        OriginalUri, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
};
use serde::Serialize;

type ApiResult<T> = Result<T, HttpApiError>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub(crate) async fn ready(State(state): State<ApiState>) -> ApiResult<Json<HealthResponse>> {
    state.queue().stats().await?;
    Ok(health().await)
}

pub(crate) async fn metrics(State(state): State<ApiState>) -> ApiResult<MetricsResponse> {
    Ok(crate::metrics::render(&state).await?)
}

pub(crate) async fn list_nodes(State(state): State<ApiState>) -> Json<Vec<NodeInfo>> {
    Json(state.node_registry().list().await)
}

pub(crate) async fn not_found(OriginalUri(uri): OriginalUri) -> HttpApiError {
    HttpApiError::not_found(format!("route {} was not found", uri.path()))
}

pub(crate) async fn create_agent(
    State(state): State<ApiState>,
    payload: Result<Json<AgentDocument>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<AgentDocument>)> {
    let document = json_payload(payload)?;
    let agent = AgentDefinition::from_document(document)
        .map_err(|error| HttpApiError::validation(error.to_string()))?;
    validate_create_metadata(agent.metadata())?;
    if agent.status().phase() != AgentPhase::Pending
        || !agent.status().conditions().is_empty()
        || agent.status().observed_version() != agent.metadata().resource_version()
    {
        return Err(HttpApiError::validation(
            "new agent status must be an empty PENDING observation",
        ));
    }
    let created = state.agents().create(agent).await?;
    Ok((StatusCode::CREATED, Json(created.into_document())))
}

pub(crate) async fn list_agents(
    State(state): State<ApiState>,
    query: Result<Query<ListOptions>, QueryRejection>,
) -> ApiResult<Json<ListResponse<AgentDocument>>> {
    let options = query_options(query)?;
    let agents = state.agents().list(None).await?;
    let (page, metadata) = paginate(agents, &options, Resource::metadata)?;
    let documents = page
        .into_iter()
        .map(AgentDefinition::into_document)
        .collect();
    Ok(Json(ListResponse::new(
        list_type("AgentList"),
        metadata,
        documents,
    )))
}

pub(crate) async fn get_agent(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDocument>> {
    let key = resource_key(&name)?;
    let agent = state
        .agents()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("agent {key} was not found")))?;
    Ok(Json(agent.into_document()))
}

pub(crate) async fn replace_agent(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    payload: Result<Json<AgentDocument>, JsonRejection>,
) -> ApiResult<Json<AgentDocument>> {
    let incoming = json_payload(payload)?;
    validate_path_metadata(&name, incoming.metadata())?;
    let key = resource_key(&name)?;
    let current = state
        .agents()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("agent {key} was not found")))?;
    let (type_meta, metadata, spec, _) = incoming.into_parts();
    let replacement = AgentDefinition::from_document(
        ResourceDocument::new(type_meta, metadata, spec).with_status(current.status().clone()),
    )
    .map_err(|error| HttpApiError::validation(error.to_string()))?;
    let persisted = state.agents().replace(replacement).await?;
    Ok(Json(persisted.into_document()))
}

pub(crate) async fn delete_agent(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let key = resource_key(&name)?;
    let current = state
        .agents()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("agent {key} was not found")))?;
    state
        .agents()
        .delete(&key, current.metadata().resource_version())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_deployment(
    State(state): State<ApiState>,
    payload: Result<Json<AgentDeploymentDocument>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<AgentDeploymentDocument>)> {
    let deployment = AgentDeployment::from_document(json_payload(payload)?)
        .map_err(|error| HttpApiError::validation(error.to_string()))?;
    validate_create_metadata(deployment.metadata())?;
    if deployment.status().observed_version() != deployment.metadata().resource_version()
        || deployment.status().ready_replicas().get() != 0
        || deployment.status().available_replicas().get() != 0
        || deployment.status().updated_replicas().get() != 0
    {
        return Err(HttpApiError::validation(
            "new deployment status must contain zero observed replicas",
        ));
    }
    let created = state.deployments().create(deployment).await?;
    Ok((StatusCode::CREATED, Json(created.into_document())))
}

pub(crate) async fn list_deployments(
    State(state): State<ApiState>,
    query: Result<Query<ListOptions>, QueryRejection>,
) -> ApiResult<Json<ListResponse<AgentDeploymentDocument>>> {
    let options = query_options(query)?;
    let deployments = state.deployments().list(None).await?;
    let (page, metadata) = paginate(deployments, &options, Resource::metadata)?;
    let documents = page
        .into_iter()
        .map(AgentDeployment::into_document)
        .collect();
    Ok(Json(ListResponse::new(
        list_type("AgentDeploymentList"),
        metadata,
        documents,
    )))
}

pub(crate) async fn get_deployment(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDeploymentDocument>> {
    let key = resource_key(&name)?;
    let deployment = state
        .deployments()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("deployment {key} was not found")))?;
    Ok(Json(deployment.into_document()))
}

pub(crate) async fn replace_deployment(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    payload: Result<Json<AgentDeploymentDocument>, JsonRejection>,
) -> ApiResult<Json<AgentDeploymentDocument>> {
    let incoming = json_payload(payload)?;
    validate_path_metadata(&name, incoming.metadata())?;
    let key = resource_key(&name)?;
    if state.deployments().get(&key).await?.is_none() {
        return Err(HttpApiError::not_found(format!(
            "deployment {key} was not found"
        )));
    }
    let (type_meta, metadata, spec, _) = incoming.into_parts();
    let document: AgentDeploymentDocument =
        ResourceDocument::without_status(type_meta, metadata, spec);
    let replacement = AgentDeployment::from_document(document)
        .map_err(|error| HttpApiError::validation(error.to_string()))?;
    let persisted = state.deployments().replace(replacement).await?;
    Ok(Json(persisted.into_document()))
}

pub(crate) async fn delete_deployment(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let key = resource_key(&name)?;
    let current = state
        .deployments()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("deployment {key} was not found")))?;
    state
        .deployments()
        .delete(&key, current.metadata().resource_version())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_task(
    State(state): State<ApiState>,
    payload: Result<Json<TaskDocument>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<TaskDocument>)> {
    let mut task = AgentTask::from_document(json_payload(payload)?)
        .map_err(|error| HttpApiError::validation(error.to_string()))?;
    validate_create_metadata(task.metadata())?;
    if task.status().state() != TaskState::Pending
        || task.status().attempts_started() != 0
        || task.status().assigned_agent().is_some()
    {
        return Err(HttpApiError::validation(
            "new task status must be an unassigned PENDING observation",
        ));
    }
    task.enqueue()
        .map_err(|error| HttpApiError::validation(error.to_string()))?;
    let request = EnqueueRequest::from_task(&task)
        .map_err(|error| HttpApiError::validation(error.to_string()))?;
    let persisted = state.tasks().create(task).await?;
    if let Err(queue_error) = state.queue().enqueue(request).await {
        let key = ResourceKey::from(persisted.metadata());
        if let Err(rollback_error) = state
            .tasks()
            .delete(&key, persisted.metadata().resource_version())
            .await
        {
            return Err(HttpApiError::internal(format!(
                "queue failed ({queue_error}) and task rollback failed ({rollback_error})"
            )));
        }
        return Err(queue_error.into());
    }
    Ok((StatusCode::CREATED, Json(persisted.into_document())))
}

pub(crate) async fn list_tasks(
    State(state): State<ApiState>,
    query: Result<Query<ListOptions>, QueryRejection>,
) -> ApiResult<Json<ListResponse<TaskDocument>>> {
    let options = query_options(query)?;
    let tasks = state.tasks().list(None).await?;
    let (page, metadata) = paginate(tasks, &options, Resource::metadata)?;
    let documents = page.into_iter().map(AgentTask::into_document).collect();
    Ok(Json(ListResponse::new(
        list_type("AgentTaskList"),
        metadata,
        documents,
    )))
}

pub(crate) async fn get_task(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<Json<TaskDocument>> {
    let key = resource_key(&name)?;
    let task = state
        .tasks()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("task {key} was not found")))?;
    Ok(Json(task.into_document()))
}

pub(crate) async fn delete_task(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let key = resource_key(&name)?;
    let task = state
        .tasks()
        .get(&key)
        .await?
        .ok_or_else(|| HttpApiError::not_found(format!("task {key} was not found")))?;
    if !task.status().state().is_terminal() {
        return Err(HttpApiError::conflict(format!(
            "task {key} cannot be deleted while it is {:?}",
            task.status().state()
        )));
    }
    state
        .tasks()
        .delete(&key, task.metadata().resource_version())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn json_payload<T>(payload: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    payload
        .map(|Json(value)| value)
        .map_err(|error| HttpApiError::bad_request(error.body_text()))
}

fn query_options(query: Result<Query<ListOptions>, QueryRejection>) -> ApiResult<ListOptions> {
    query
        .map(|Query(options)| options)
        .map_err(|error| HttpApiError::bad_request(error.body_text()))
}

fn resource_key(name: &str) -> ApiResult<ResourceKey> {
    let name =
        ResourceName::new(name).map_err(|error| HttpApiError::bad_request(error.to_string()))?;
    Ok(ResourceKey::new(Namespace::default(), name))
}

fn validate_create_metadata(metadata: &Metadata) -> ApiResult<()> {
    if metadata.namespace() != &Namespace::default() {
        return Err(HttpApiError::validation(
            "v1 endpoints currently accept only the default namespace",
        ));
    }
    Ok(())
}

fn validate_path_metadata(path_name: &str, metadata: &Metadata) -> ApiResult<()> {
    validate_create_metadata(metadata)?;
    if metadata.name().as_str() != path_name {
        return Err(HttpApiError::validation(format!(
            "path name {path_name:?} does not match metadata.name {:?}",
            metadata.name().as_str()
        )));
    }
    Ok(())
}

fn list_type(kind: &str) -> TypeMeta {
    TypeMeta::new(
        ApiVersion::new("agentkube.ai/v1").expect("static API version is valid"),
        ResourceKind::new(kind).expect("static list kind is valid"),
    )
}
