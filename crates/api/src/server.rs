use crate::{ApiState, handlers};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use std::io;
use tokio::net::TcpListener;

/// Builds the versioned AgentKube HTTP router.
pub fn router(state: ApiState, max_request_body_bytes: usize) -> Router {
    Router::new()
        .route("/healthz", get(handlers::health))
        .route("/readyz", get(handlers::ready))
        .route(
            "/v1/agents",
            post(handlers::create_agent).get(handlers::list_agents),
        )
        .route(
            "/v1/agents/{name}",
            get(handlers::get_agent)
                .put(handlers::replace_agent)
                .delete(handlers::delete_agent),
        )
        .route(
            "/v1/deployments",
            post(handlers::create_deployment).get(handlers::list_deployments),
        )
        .route(
            "/v1/deployments/{name}",
            get(handlers::get_deployment)
                .put(handlers::replace_deployment)
                .delete(handlers::delete_deployment),
        )
        .route(
            "/v1/tasks",
            post(handlers::create_task).get(handlers::list_tasks),
        )
        .route(
            "/v1/tasks/{name}",
            get(handlers::get_task).delete(handlers::delete_task),
        )
        .fallback(handlers::not_found)
        .layer(DefaultBodyLimit::max(max_request_body_bytes))
        .with_state(state)
}

/// Serves the API until the returned future is cancelled or the listener fails.
pub async fn serve(listener: TcpListener, application: Router) -> io::Result<()> {
    axum::serve(listener, application).await
}
