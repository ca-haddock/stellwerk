use axum::{
    routing::{get, post, put},
    Router,
};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use crate::monitor::StatusRef;

pub mod routes;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub status: StatusRef,
    pub default_gw: String,
    pub scan_tx: Arc<tokio::sync::Notify>,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::serve_ui))
        .route("/api/status", get(routes::get_status))
        .route("/api/clients", get(routes::list_clients))
        .route("/api/clients/:ip", get(routes::get_client))
        .route("/api/clients/:ip/gateway", put(routes::set_gateway))
        .route("/api/clients/:ip/label", put(routes::set_label))
        .route("/api/gateways", get(routes::list_gateways))
        .route("/api/events", get(routes::list_events))
        .route("/api/scan", post(routes::trigger_scan))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
