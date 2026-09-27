use crate::{ApiState, HttpApiError, handlers};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::io;
use tokio::net::TcpListener;

/// Builds the versioned AgentKube HTTP router.
///
/// Versioned endpoints enforce Bearer authentication when the state carries
/// an auth token; liveness, readiness, and metrics stay open for probes and
/// scrapers.
pub fn router(state: ApiState, max_request_body_bytes: usize) -> Router {
    let versioned = Router::new()
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
        .route("/v1/nodes", get(handlers::list_nodes))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .route("/healthz", get(handlers::health))
        .route("/readyz", get(handlers::ready))
        .route("/metrics", get(handlers::metrics))
        .merge(versioned)
        .fallback(handlers::not_found)
        .layer(DefaultBodyLimit::max(max_request_body_bytes))
        .with_state(state)
}

async fn require_auth(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.auth_token() else {
        return next.run(request).await;
    };
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if authorized {
        next.run(request).await
    } else {
        HttpApiError::unauthorized("authentication required").into_response()
    }
}

/// Serves the API until the returned future is cancelled or the listener fails.
pub async fn serve(listener: TcpListener, application: Router) -> io::Result<()> {
    axum::serve(listener, application).await
}
