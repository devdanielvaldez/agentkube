//! Point-in-time Prometheus exposition for operator scraping.
//!
//! Metrics are computed on demand from repository and queue state, so no
//! instrumentation code paths or additional dependencies are required.

use crate::{ApiState, HttpApiError};
use agentkube_tasks::TaskState;
use axum::{
    http::{HeaderMap, HeaderValue, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use std::fmt::Write as _;

/// Prometheus exposition of one scrape.
pub(crate) struct MetricsResponse(String);

impl IntoResponse for MetricsResponse {
    fn into_response(self) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4"),
        );
        (headers, self.0).into_response()
    }
}

/// Renders the `/metrics` exposition from live control-plane state.
pub(crate) async fn render(state: &ApiState) -> Result<MetricsResponse, HttpApiError> {
    let agents = state.agents().list(None).await?;
    let deployments = state.deployments().list(None).await?;
    let tasks = state.tasks().list(None).await?;
    let queue = state.queue().stats().await?;
    let nodes = state.node_registry().list().await.len();

    let mut states = [0u64; 10];
    for task in &tasks {
        states[state_index(task.status().state())] += 1;
    }
    let desired: u64 = deployments
        .iter()
        .map(|deployment| u64::from(deployment.spec().replicas().get()))
        .sum();

    let mut body = String::new();
    let _ = writeln!(
        body,
        "# HELP agentkube_build_info Control-plane build metadata.\n\
         # TYPE agentkube_build_info gauge\n\
         agentkube_build_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(
        body,
        "# HELP agentkube_uptime_seconds Seconds since process start.\n\
         # TYPE agentkube_uptime_seconds counter\n\
         agentkube_uptime_seconds {}",
        state.uptime_secs()
    );
    gauge(
        &mut body,
        "agentkube_agents_total",
        "Stored agent definitions.",
        agents.len() as u64,
    );
    gauge(
        &mut body,
        "agentkube_deployments_total",
        "Stored agent deployments.",
        deployments.len() as u64,
    );
    gauge(
        &mut body,
        "agentkube_deployment_desired_replicas",
        "Sum of desired replicas across deployments.",
        desired,
    );
    gauge(
        &mut body,
        "agentkube_tasks_total",
        "Stored agent tasks.",
        tasks.len() as u64,
    );
    for (index, name) in state_names().iter().enumerate() {
        gauge(
            &mut body,
            &format!("agentkube_tasks_{}", name.to_ascii_lowercase()),
            &format!("Stored tasks in {name} state."),
            states[index],
        );
    }
    gauge(
        &mut body,
        "agentkube_queue_ready",
        "Queue entries available for delivery.",
        queue.ready() as u64,
    );
    gauge(
        &mut body,
        "agentkube_queue_delayed",
        "Queue entries waiting for visibility.",
        queue.delayed() as u64,
    );
    gauge(
        &mut body,
        "agentkube_queue_leased",
        "Queue entries leased to consumers.",
        queue.leased() as u64,
    );
    gauge(
        &mut body,
        "agentkube_nodes_total",
        "Worker nodes with a recorded heartbeat.",
        nodes as u64,
    );
    Ok(MetricsResponse(body))
}

fn gauge(body: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(
        body,
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
    );
}

fn state_index(state: TaskState) -> usize {
    match state {
        TaskState::Pending => 0,
        TaskState::Queued => 1,
        TaskState::Scheduled => 2,
        TaskState::Running => 3,
        TaskState::WaitingTool => 4,
        TaskState::WaitingAgent => 5,
        TaskState::WaitingApproval => 6,
        TaskState::Completed => 7,
        TaskState::Failed => 8,
        TaskState::Cancelled => 9,
    }
}

fn state_names() -> [&'static str; 10] {
    [
        "PENDING",
        "QUEUED",
        "SCHEDULED",
        "RUNNING",
        "WAITING_TOOL",
        "WAITING_AGENT",
        "WAITING_APPROVAL",
        "COMPLETED",
        "FAILED",
        "CANCELLED",
    ]
}
