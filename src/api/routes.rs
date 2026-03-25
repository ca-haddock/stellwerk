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
        "version": env!("CARGO_PKG_VERSION"),
        "ppp0_up": s.ppp0_up,
        "gre_up": s.gre_up,
        "last_check": s.last_check,
        "recent_events": events,
        "default_gw": state.default_gw,
        "scan_subnets": state.scan_subnets,
        "dns_servers": state.dns_servers,
        "mullvad_configured": state.mullvad_config.is_some(),
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

#[derive(Deserialize)]
pub struct SetGroupBody {
    pub group_name: String,
}

pub async fn set_group(
    State(state): State<AppState>,
    Path(ip): Path<String>,
    Json(body): Json<SetGroupBody>,
) -> impl IntoResponse {
    match db::set_client_group(&state.pool, &ip, &body.group_name).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetAutofallbackBody {
    pub autofallback: bool,
}

pub async fn set_autofallback(
    State(state): State<AppState>,
    Path(ip): Path<String>,
    Json(body): Json<SetAutofallbackBody>,
) -> impl IntoResponse {
    match db::set_client_autofallback(&state.pool, &ip, body.autofallback).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true, "autofallback": body.autofallback }))).into_response(),
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

#[derive(Deserialize)]
pub struct SetGatewayDnsBody {
    pub dns_ip: String,
}

pub async fn set_gateway_dns(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<SetGatewayDnsBody>,
) -> impl IntoResponse {
    let dns_ip = if body.dns_ip.trim().is_empty() { None } else { Some(body.dns_ip.trim()) };
    match db::set_gateway_dns_ip(&state.pool, &name, dns_ip).await {
        Ok(true) => {
            state.scan_tx.notify_one();
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "gateway not found" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetClientDnsBody {
    pub dns_ip: String,
}

pub async fn set_client_dns(
    State(state): State<AppState>,
    Path(ip): Path<String>,
    Json(body): Json<SetClientDnsBody>,
) -> impl IntoResponse {
    let dns_ip = if body.dns_ip.trim().is_empty() { None } else { Some(body.dns_ip.trim()) };
    match db::set_client_dns_ip(&state.pool, &ip, dns_ip).await {
        Ok(true) => {
            state.scan_tx.notify_one();
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn get_settings(State(state): State<AppState>) -> Json<Value> {
    let unbound_gw = db::get_setting(&state.pool, "unbound-gateway").await.unwrap_or(None).unwrap_or_default();
    Json(json!({ "unbound_gateway": unbound_gw }))
}

#[derive(Deserialize)]
pub struct SetSettingBody {
    pub value: String,
}

pub async fn set_setting(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<SetSettingBody>,
) -> impl IntoResponse {
    match db::set_setting(&state.pool, &key, &body.value).await {
        Ok(()) => {
            state.scan_tx.notify_one();
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// --- Traffic ---

pub async fn get_traffic(State(state): State<AppState>) -> Json<Value> {
    match db::get_traffic_24h(&state.pool).await {
        Ok(records) => Json(json!(records)),
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

// --- Interfaces ---

pub async fn list_interfaces(State(state): State<AppState>) -> Json<Value> {
    // Read interface names from /proc/net/dev
    let iface_names: Vec<String> = std::fs::read_to_string("/proc/net/dev")
        .unwrap_or_default()
        .lines()
        .skip(2)
        .filter_map(|line| {
            let name = line.split(':').next()?.trim().to_string();
            if name == "lo" { return None; }
            Some(name)
        })
        .collect();

    let meta_list = db::list_interface_meta(&state.pool).await.unwrap_or_default();
    let meta_map: std::collections::HashMap<_, _> = meta_list.into_iter()
        .map(|m| (m.name.clone(), m))
        .collect();

    let gateways = db::list_gateways(&state.pool).await.unwrap_or_default();
    let mut iface_gateways: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for gw in &gateways {
        iface_gateways.entry(gw.interface.clone()).or_default().push(gw.name.clone());
    }

    let result: Vec<Value> = iface_names.iter().map(|name| {
        let meta = meta_map.get(name);
        json!({
            "name": name,
            "role": meta.map(|m| m.role.as_str()).unwrap_or("extern"),
            "enabled": meta.map(|m| m.enabled).unwrap_or(1),
            "gateways": iface_gateways.get(name).cloned().unwrap_or_default(),
        })
    }).collect();

    Json(json!(result))
}

#[derive(Deserialize)]
pub struct SetInterfaceBody {
    pub role: String,
    pub enabled: bool,
}

pub async fn set_interface_meta(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<SetInterfaceBody>,
) -> impl IntoResponse {
    match db::upsert_interface_meta(&state.pool, &name, &body.role, body.enabled).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// --- Networks ---

pub async fn list_networks(State(state): State<AppState>) -> Json<Value> {
    match db::list_networks(&state.pool).await {
        Ok(nets) => Json(json!(nets)),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

#[derive(Deserialize)]
pub struct SetNetworkBody {
    pub default_gateway: String,
    pub internal_only: bool,
    pub gateway_only: bool,
    #[serde(default)]
    pub dns_ip: String,
}

pub async fn set_network(
    State(state): State<AppState>,
    Path(subnet): Path<String>,
    Json(body): Json<SetNetworkBody>,
) -> impl IntoResponse {
    let dns_ip = if body.dns_ip.trim().is_empty() { None } else { Some(body.dns_ip.trim()) };
    match db::upsert_network(&state.pool, &subnet, &body.default_gateway, body.internal_only, body.gateway_only, dns_ip).await {
        Ok(()) => {
            state.scan_tx.notify_one();
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
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
        return (StatusCode::OK, Json(json!({"ok": true, "role": "admin"}))).into_response();
    }
    let hash = crate::auth::hash_password(&body.password);

    let (is_admin, is_viewer) = (
        body.username == state.username && hash == state.password_hash,
        !state.viewer_username.is_empty()
            && body.username == state.viewer_username
            && hash == state.viewer_password_hash,
    );

    if !is_admin && !is_viewer {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Ungültige Zugangsdaten"}))).into_response();
    }

    let token = crate::auth::generate_token();
    let role = if is_admin {
        state.sessions.write().await.insert(token.clone());
        "admin"
    } else {
        state.viewer_sessions.write().await.insert(token.clone());
        "viewer"
    };

    let cookie = format!("session={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=86400", token);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({"ok": true, "role": role})),
    ).into_response()
}

pub async fn get_me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    if !state.auth_enabled {
        return Json(json!({"role": "admin"}));
    }
    if let Some(token) = crate::auth::extract_session_token(&headers) {
        if state.sessions.read().await.contains(&token) {
            return Json(json!({"role": "admin"}));
        }
        if state.viewer_sessions.read().await.contains(&token) {
            return Json(json!({"role": "viewer"}));
        }
    }
    Json(json!({"role": "unknown"}))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Some(token) = crate::auth::extract_session_token(&headers) {
        state.sessions.write().await.remove(&token);
        state.viewer_sessions.write().await.remove(&token);
    }
    let cookie = "session=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0";
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({"ok": true})),
    ).into_response()
}

// --- Kiosk ---

pub async fn kiosk_login(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> impl IntoResponse {
    if !state.auth_enabled || state.kiosk_token.is_empty() {
        return axum::response::Redirect::to("/").into_response();
    }
    if token != state.kiosk_token {
        return (StatusCode::FORBIDDEN, Html("<h1>403 Forbidden</h1>")).into_response();
    }
    let session_token = crate::auth::generate_token();
    state.viewer_sessions.write().await.insert(session_token.clone());
    let cookie = format!("session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=315360000", session_token);
    (
        StatusCode::FOUND,
        [(header::SET_COOKIE, cookie), (header::LOCATION, "/".to_string())],
    ).into_response()
}

// --- Scan ---

pub async fn trigger_scan(State(state): State<AppState>) -> Json<Value> {
    state.scan_tx.notify_one();
    Json(json!({ "ok": true, "message": "Scan triggered" }))
}

// --- WireGuard Interface Sync ---

/// Synchronisiert aktive WireGuard-Interfaces mit der Gateway-Datenbank.
/// Neue Interfaces → Gateway anlegen + Routing einrichten.
/// Verschwundene Interfaces → Gateway entfernen.
/// Mullvad-Interfaces (mu*) werden übersprungen (eigener Flow).
pub async fn wg_sync(State(state): State<AppState>) -> Json<Value> {
    let active_ifaces = crate::mullvad::list_wg_interfaces().await;
    let gateways = match db::list_gateways(&state.pool).await {
        Ok(g) => g,
        Err(e) => return Json(json!({ "error": e.to_string() })),
    };

    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();

    // Neue Interfaces → Gateway anlegen
    for iface in &active_ifaces {
        // Mullvad-Interfaces überspringen (werden via mullvad-connect verwaltet)
        if crate::mullvad::is_mullvad_interface(iface) {
            continue;
        }
        // Schon vorhanden?
        if gateways.iter().any(|g| &g.interface == iface) {
            continue;
        }
        // Routing-Tabelle + Mark zuweisen
        let table_num = crate::mullvad::next_free_table_number().await;
        let mark = crate::mullvad::next_free_mark(&state.pool).await;
        let description = format!("WireGuard: {}", iface);
        if let Err(e) = crate::mullvad::add_rt_table_entry_for(iface, table_num).await {
            tracing::warn!("rt_tables Eintrag für {} fehlgeschlagen: {}", iface, e);
            continue;
        }
        if let Err(e) = db::upsert_gateway(&state.pool, iface, iface, iface, None, &description, mark, None).await {
            tracing::warn!("Gateway {} anlegen fehlgeschlagen: {}", iface, e);
            continue;
        }
        // Default-Route in die Interface-Tabelle eintragen
        crate::mullvad::add_default_route_for(iface, iface).await;
        // Interface als extern markieren
        let _ = db::upsert_interface_meta(&state.pool, iface, "extern", true).await;
        added.push(iface.clone());
        tracing::info!("WireGuard-Sync: Gateway '{}' angelegt (table {})", iface, table_num);
    }

    // Verschwundene Interfaces → Gateway entfernen
    // Nur Gateways wo name == interface (auto-angelegte), nicht Mullvad, nicht seeded
    for gw in &gateways {
        if crate::mullvad::is_mullvad_interface(&gw.interface) {
            continue;
        }
        // Nur auto-angelegte: name == interface
        if gw.name != gw.interface {
            continue;
        }
        // Noch aktiv?
        if active_ifaces.contains(&gw.interface) {
            continue;
        }
        // Interface existiert nicht mehr → aufräumen
        if let Err(e) = crate::mullvad::remove_rt_table_entry_for(&gw.interface).await {
            tracing::warn!("rt_tables Entfernen für {} fehlgeschlagen: {}", gw.interface, e);
        }
        // Clients auf Default-Gateway umleiten
        if let Ok(clients) = db::list_clients(&state.pool).await {
            for c in clients.iter().filter(|c| c.gateway == gw.name) {
                let _ = db::set_client_gateway(&state.pool, &c.ip, &state.default_gw).await;
            }
        }
        let _ = sqlx::query("DELETE FROM gateways WHERE name = ?1")
            .bind(&gw.name)
            .execute(&state.pool)
            .await;
        let _ = sqlx::query("DELETE FROM interface_meta WHERE name = ?1")
            .bind(&gw.interface)
            .execute(&state.pool)
            .await;
        removed.push(gw.name.clone());
        tracing::info!("WireGuard-Sync: Gateway '{}' entfernt", gw.name);
    }

    state.scan_tx.notify_one();
    Json(json!({ "ok": true, "added": added, "removed": removed }))
}

// --- Mullvad ---

#[derive(Deserialize)]
pub struct CreateDeviceBody {
    pub name: String,
}

/// Erstellt ein benanntes WireGuard-Gerät: generiert Keypair, registriert bei Mullvad, speichert in DB.
pub async fn mullvad_create_device(
    State(state): State<AppState>,
    Json(body): Json<CreateDeviceBody>,
) -> impl IntoResponse {
    let Some(mullvad_cfg) = &state.mullvad_config else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Kein [mullvad] Block in config.toml" }))).into_response();
    };
    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Gerätename muss 1–64 Zeichen lang sein" }))).into_response();
    }
    match db::get_mullvad_device(&state.pool, &name).await {
        Ok(Some(_)) => return (StatusCode::CONFLICT, Json(json!({ "error": format!("Gerät '{}' existiert bereits", name) }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
        _ => {}
    }
    let account = mullvad_cfg.account.clone();
    let (private_key, public_key) = match crate::mullvad::generate_keypair().await {
        Ok(kp) => kp,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let address = match crate::mullvad::register_key(&account, &public_key, Some(&name)).await {
        Ok(addr) => addr,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    if let Err(e) = db::insert_mullvad_device(&state.pool, &name, &private_key, &public_key, &address).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }
    (StatusCode::OK, Json(json!({
        "ok": true,
        "name": name,
        "public_key": public_key,
        "address": address,
    }))).into_response()
}

/// Listet alle gespeicherten Mullvad-Geräte (ohne private keys).
pub async fn mullvad_list_devices(State(state): State<AppState>) -> Json<Value> {
    match db::list_mullvad_devices(&state.pool).await {
        Ok(devices) => {
            let result: Vec<_> = devices.iter().map(|d| json!({
                "name": d.name,
                "public_key": d.public_key,
                "address": d.address,
                "created_at": d.created_at,
            })).collect();
            Json(json!(result))
        }
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// Löscht ein Mullvad-Gerät und deregistriert den Key (best-effort).
pub async fn mullvad_delete_device(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let device = match db::get_mullvad_device(&state.pool, &name).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "Gerät nicht gefunden" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    if let Some(mullvad_cfg) = &state.mullvad_config {
        if let Err(e) = crate::mullvad::deregister_key(&mullvad_cfg.account, &device.public_key).await {
            tracing::warn!("Mullvad Key-Deregistrierung fehlgeschlagen (wird trotzdem gelöscht): {}", e);
        }
    }
    match db::delete_mullvad_device(&state.pool, &name).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// Deprecated: Einzelner globaler Key. Verwende POST /api/mullvad/devices stattdessen.
pub async fn mullvad_setup(State(_state): State<AppState>) -> impl IntoResponse {
    (StatusCode::GONE, Json(json!({
        "error": "Deprecated. Bitte POST /api/mullvad/devices mit {\"name\": \"...\"} verwenden.",
        "deprecated": true,
    }))).into_response()
}

/// Deprecated: Key-Status. Wird durch mullvad_list_devices ersetzt.
pub async fn mullvad_key_status(State(state): State<AppState>) -> Json<Value> {
    let devices = db::list_mullvad_devices(&state.pool).await.unwrap_or_default();
    Json(json!({
        "has_key": !devices.is_empty(),
        "deprecated": true,
    }))
}

pub async fn mullvad_countries() -> impl IntoResponse {
    match crate::mullvad::fetch_countries().await {
        Ok(countries) => (StatusCode::OK, Json(json!(countries))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn mullvad_connections(State(state): State<AppState>) -> Json<Value> {
    match db::list_gateways(&state.pool).await {
        Ok(gws) => {
            let conns: Vec<_> = gws.iter()
                .filter(|g| crate::mullvad::is_mullvad_interface(&g.interface))
                .map(|g| {
                    let cc = g.interface.trim_start_matches("mu");
                    json!({
                        "country_code": cc,
                        "name": g.name,
                        "interface": g.interface,
                        "description": g.description,
                        "device_name": g.device_name,
                    })
                })
                .collect();
            Json(json!(conns))
        }
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

#[derive(Deserialize)]
pub struct MullvadConnectBody {
    pub device_name: String,
    pub country_code: String,
}

pub async fn mullvad_connect(
    State(state): State<AppState>,
    Json(body): Json<MullvadConnectBody>,
) -> impl IntoResponse {
    let cc = body.country_code.to_lowercase();
    let cc = cc.trim();
    let device_name = body.device_name.trim().to_string();

    if state.mullvad_config.is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Mullvad nicht konfiguriert (fehlender [mullvad] Block in config.toml)" }))).into_response();
    }

    // Load device credentials from DB
    let device = match db::get_mullvad_device(&state.pool, &device_name).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("Gerät '{}' nicht gefunden. Bitte zuerst ein Gerät erstellen.", device_name) }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };

    // Check if already connected
    if crate::mullvad::is_up(cc).await {
        return (StatusCode::CONFLICT, Json(json!({ "error": format!("mu{} läuft bereits", cc) }))).into_response();
    }

    // Fetch best relay for country
    let relays = match crate::mullvad::fetch_relays_for_country(cc).await {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("Keine aktiven Mullvad-Server für Land: {}", cc) }))).into_response(),
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let relay = &relays[0];

    // Generate and write WireGuard config
    let wg_config = crate::mullvad::generate_wg_config(&device.private_key, &device.address, relay);
    if let Err(e) = crate::mullvad::write_wg_config(cc, &wg_config).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }

    // Reserve table number + mark
    let table_num = crate::mullvad::next_free_table_number().await;
    let mark = crate::mullvad::next_free_mark(&state.pool).await;

    // Add rt_tables entry
    if let Err(e) = crate::mullvad::add_rt_table_entry(cc, table_num).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }

    // Bring up interface
    if let Err(e) = crate::mullvad::bring_up(cc).await {
        let _ = crate::mullvad::remove_rt_table_entry(cc).await;
        crate::mullvad::remove_wg_config(cc).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }

    // Add default route
    crate::mullvad::add_default_route(cc).await;

    // Register gateway in DB
    let iface = crate::mullvad::interface_name(cc);
    let gw_name = iface.clone();
    let description = format!("Mullvad {} ({})", cc.to_uppercase(), device_name);
    if let Err(e) = db::upsert_gateway(&state.pool, &gw_name, &gw_name, &iface, None, &description, mark, Some(&device_name)).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }

    // Mullvad-eigener DNS-Server für DNS-Leak-Schutz automatisch setzen
    let _ = db::set_gateway_dns_ip(&state.pool, &gw_name, Some("10.64.0.1")).await;

    // Interface als extern taggen → erscheint sofort im Gateway-Dropdown
    let _ = db::upsert_interface_meta(&state.pool, &iface, "extern", true).await;

    state.scan_tx.notify_one();
    (StatusCode::OK, Json(json!({
        "ok": true,
        "name": gw_name,
        "interface": iface,
        "device_name": device_name,
        "server": relay.hostname,
        "endpoint": relay.ipv4_addr_in,
    }))).into_response()
}

pub async fn mullvad_disconnect(
    State(state): State<AppState>,
    Path(cc): Path<String>,
) -> impl IntoResponse {
    let cc = cc.to_lowercase();
    let cc = cc.trim();
    let gw_name = crate::mullvad::interface_name(cc);

    // Remove clients from this gateway → fallback to default
    if let Ok(clients) = db::list_clients(&state.pool).await {
        for c in clients.iter().filter(|c| c.gateway == gw_name) {
            let _ = db::set_client_gateway(&state.pool, &c.ip, &state.default_gw).await;
        }
    }

    // Bring down interface
    crate::mullvad::bring_down(cc).await.ok();

    // Remove gateway from DB
    let _ = sqlx::query("DELETE FROM gateways WHERE name = ?1")
        .bind(&gw_name)
        .execute(&state.pool)
        .await;

    // Remove rt_tables entry + WireGuard config
    let _ = crate::mullvad::remove_rt_table_entry(cc).await;
    crate::mullvad::remove_wg_config(cc).await;

    // Interface-Meta entfernen
    let _ = sqlx::query("DELETE FROM interface_meta WHERE name = ?1")
        .bind(&gw_name)
        .execute(&state.pool)
        .await;

    state.scan_tx.notify_one();
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}
