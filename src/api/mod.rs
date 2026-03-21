use axum::{
    extract::State,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use crate::auth::Sessions;
use crate::monitor::StatusRef;

pub mod routes;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub status: StatusRef,
    pub default_gw: String,
    pub scan_tx: Arc<tokio::sync::Notify>,
    pub sessions: Sessions,
    pub auth_enabled: bool,
    pub username: String,
    pub password_hash: String,
}

async fn require_auth(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> impl IntoResponse {
    if !state.auth_enabled {
        return next.run(request).await;
    }
    if let Some(token) = crate::auth::extract_session_token(request.headers()) {
        if state.sessions.read().await.contains(&token) {
            return next.run(request).await;
        }
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response()
}

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/status", get(routes::get_status))
        .route("/api/clients", get(routes::list_clients))
        .route("/api/clients/:ip", get(routes::get_client))
        .route("/api/clients/:ip/gateway", put(routes::set_gateway))
        .route("/api/clients/:ip/label", put(routes::set_label))
        .route("/api/gateways", get(routes::list_gateways))
        .route("/api/events", get(routes::list_events))
        .route("/api/scan", post(routes::trigger_scan))
        .route("/api/logout", post(routes::logout))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/", get(routes::serve_ui))
        .route("/api/login", post(routes::login))
        .merge(protected)
        .layer(CorsLayer::permissive())
        .with_state(state)
}
