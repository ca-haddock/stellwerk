use axum::{
    extract::State,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::{delete, get, post, put},
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
    pub scan_subnets: Vec<String>,
    pub scan_tx: Arc<tokio::sync::Notify>,
    pub sessions: Sessions,
    pub viewer_sessions: Sessions,
    pub auth_enabled: bool,
    pub username: String,
    pub password_hash: String,
    pub viewer_username: String,
    pub viewer_password_hash: String,
    pub kiosk_token: String,
    /// Benannte DNS-Server aus config.toml [dns.servers], alphabetisch sortiert
    pub dns_servers: Vec<(String, String)>,
    /// Mullvad-Konfiguration (private key + address) aus config.toml
    pub mullvad_config: Option<crate::config::MullvadConfig>,
    /// HomeAssistant-Client für Stargate-Steuerung (optional)
    pub ha_client: Option<crate::homeassistant::HomeAssistantClient>,
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
        let is_admin = state.sessions.read().await.contains(&token);
        let is_viewer = state.viewer_sessions.read().await.contains(&token);
        if is_admin || is_viewer {
            return next.run(request).await;
        }
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response()
}

async fn require_write(
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
    (StatusCode::FORBIDDEN, Json(json!({"error": "read-only"}))).into_response()
}

pub fn build_router(state: AppState) -> Router {
    let read_only = Router::new()
        .route("/api/status", get(routes::get_status))
        .route("/api/stargate/status", get(routes::get_stargate_status))
        .route("/api/me", get(routes::get_me))
        .route("/api/clients", get(routes::list_clients))
        .route("/api/clients/:ip", get(routes::get_client))
        .route("/api/gateways", get(routes::list_gateways))
        .route("/api/traffic", get(routes::get_traffic))
        .route("/api/events", get(routes::list_events))
        .route("/api/ifaces", get(routes::list_interfaces))
        .route("/api/networks", get(routes::list_networks))
        .route("/api/settings", get(routes::get_settings))
        .route("/api/mullvad/countries", get(routes::mullvad_countries))
        .route("/api/mullvad/connections", get(routes::mullvad_connections))
        .route("/api/mullvad/key", get(routes::mullvad_key_status))
        .route("/api/mullvad/devices", get(routes::mullvad_list_devices))
        .route("/api/logout", post(routes::logout))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let write_only = Router::new()
        .route("/api/clients/:ip/gateway", put(routes::set_gateway))
        .route("/api/clients/:ip/label", put(routes::set_label))
        .route("/api/clients/:ip/group", put(routes::set_group))
        .route("/api/clients/:ip/autofallback", put(routes::set_autofallback))
        .route("/api/scan", post(routes::trigger_scan))
        .route("/api/ifaces/:name", put(routes::set_interface_meta))
        .route("/api/networks/:subnet", put(routes::set_network))
        .route("/api/gateways/:name/dns", put(routes::set_gateway_dns))
        .route("/api/clients/:ip/dns", put(routes::set_client_dns))
        .route("/api/settings/:key", put(routes::set_setting))
        .route("/api/wg/sync", post(routes::wg_sync))
        .route("/api/mullvad/setup", post(routes::mullvad_setup))
        .route("/api/mullvad/devices", post(routes::mullvad_create_device))
        .route("/api/mullvad/devices/:name", delete(routes::mullvad_delete_device))
        .route("/api/mullvad/connect", post(routes::mullvad_connect))
        .route("/api/mullvad/:cc", delete(routes::mullvad_disconnect))
        .route("/api/stargate/on", post(routes::stargate_on))
        .route("/api/stargate/off", post(routes::stargate_off))
        .layer(middleware::from_fn_with_state(state.clone(), require_write));

    Router::new()
        .route("/", get(routes::serve_ui))
        .route("/kiosk/:token", get(routes::kiosk_login))
        .route("/api/login", post(routes::login))
        .merge(read_only)
        .merge(write_only)
        .layer(CorsLayer::permissive())
        .with_state(state)
}
