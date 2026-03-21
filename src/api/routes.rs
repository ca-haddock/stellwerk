use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::AppState;
use crate::db;

// --- Static UI ---

pub async fn serve_ui() -> Html<&'static str> {
    Html(include_str!("../../web/index.html"))
}

// --- Status ---

pub async fn get_status(State(state): State<AppState>) -> Json<Value> {
    let s = state.status.read().await;
    let events = db::list_recent_events(&state.pool, 20)
        .await
        .unwrap_or_default();

    Json(json!({
        "ppp0_up": s.ppp0_up,
        "gre_up": s.gre_up,
        "last_check": s.last_check,
        "recent_events": events,
    }))
}

// --- Clients ---

pub async fn list_clients(State(state): State<AppState>) -> Json<Value> {
    match db::list_clients(&state.pool).await {
        Ok(clients) => Json(json!(clients)),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

pub async fn get_client(
    State(state): State<AppState>,
    Path(ip): Path<String>,
) -> impl IntoResponse {
    match db::get_client(&state.pool, &ip).await {
        Ok(Some(client)) => (StatusCode::OK, Json(json!(client))).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetGatewayBody {
    pub gateway: String,
}

pub async fn set_gateway(
    State(state): State<AppState>,
    Path(ip): Path<String>,
    Json(body): Json<SetGatewayBody>,
) -> impl IntoResponse {
    // Validate gateway exists
    match db::get_gateway(&state.pool, &body.gateway).await {
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "unknown gateway" })),
            ).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            ).into_response();
        }
        Ok(Some(_)) => {}
    }

    match db::set_client_gateway(&state.pool, &ip, &body.gateway).await {
        Ok(true) => {
            // Trigger routing reapply in background
            state.scan_tx.notify_one();
            (StatusCode::OK, Json(json!({ "ok": true, "ip": ip, "gateway": body.gateway }))).into_response()
        }
        Ok(false) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "client not found" }))).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct SetLabelBody {
    pub label: String,
}

pub async fn set_label(
    State(state): State<AppState>,
    Path(ip): Path<String>,
    Json(body): Json<SetLabelBody>,
) -> impl IntoResponse {
    match db::set_client_label(&state.pool, &ip, &body.label).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// --- Gateways ---

pub async fn list_gateways(State(state): State<AppState>) -> Json<Value> {
    match db::list_gateways(&state.pool).await {
        Ok(gws) => Json(json!(gws)),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

// --- Events ---

pub async fn list_events(State(state): State<AppState>) -> Json<Value> {
    match db::list_recent_events(&state.pool, 50).await {
        Ok(events) => Json(json!(events)),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

// --- Auth ---

#[derive(Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    if !state.auth_enabled {
        return (StatusCode::OK, Json(json!({"ok": true}))).into_response();
    }
    let hash = crate::auth::hash_password(&body.password);
    if body.username != state.username || hash != state.password_hash {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Ungültige Zugangsdaten"}))).into_response();
    }
    let token = crate::auth::generate_token();
    state.sessions.write().await.insert(token.clone());
    let cookie = format!("session={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=86400", token);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({"ok": true})),
    ).into_response()
}

pub async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Some(token) = crate::auth::extract_session_token(&headers) {
        state.sessions.write().await.remove(&token);
    }
    let cookie = "session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0";
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({"ok": true})),
    ).into_response()
}

// --- Scan ---

pub async fn trigger_scan(State(state): State<AppState>) -> Json<Value> {
    state.scan_tx.notify_one();
    Json(json!({ "ok": true, "message": "Scan triggered" }))
}
