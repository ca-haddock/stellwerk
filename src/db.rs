use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Client {
    pub ip: String,
    pub mac: Option<String>,
    pub hostname: Option<String>,
    pub label: Option<String>,
    pub gateway: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Gateway {
    pub name: String,
    pub table_name: String,
    pub interface: String,
    pub src_ip: Option<String>,
    pub description: Option<String>,
    pub mark: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MonitorEvent {
    pub id: i64,
    pub ts: i64,
    pub event: String,
    pub detail: Option<String>,
}

pub async fn init_pool(db_path: &str) -> Result<SqlitePool> {
    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create db directory: {}", parent.display()))?;
    }

    let opts = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_path))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

    let pool = SqlitePool::connect_with(opts)
        .await
        .with_context(|| format!("Failed to open SQLite database: {}", db_path))?;

    run_migrations(&pool).await?;
    seed_gateways(&pool).await?;

    Ok(pool)
}

async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS clients (
            ip         TEXT PRIMARY KEY,
            mac        TEXT,
            hostname   TEXT,
            label      TEXT,
            gateway    TEXT NOT NULL DEFAULT 'gre_175',
            first_seen INTEGER NOT NULL,
            last_seen  INTEGER NOT NULL,
            active     INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS gateways (
            name        TEXT PRIMARY KEY,
            table_name  TEXT NOT NULL,
            interface   TEXT NOT NULL,
            src_ip      TEXT,
            description TEXT,
            mark        INTEGER UNIQUE NOT NULL
        );
        CREATE TABLE IF NOT EXISTS monitor_events (
            id     INTEGER PRIMARY KEY AUTOINCREMENT,
            ts     INTEGER NOT NULL,
            event  TEXT NOT NULL,
            detail TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_clients_last_seen ON clients(last_seen);
        CREATE INDEX IF NOT EXISTS idx_monitor_events_ts ON monitor_events(ts DESC);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_gateways(pool: &SqlitePool) -> Result<()> {
    let gateways: Vec<(&str, &str, &str, Option<&str>, &str, i64)> = vec![
        ("gre_175", "gre_175", "gre_fiber", Some("5.230.119.175"), "GRE Fiber – öffentliche IP .175 (Standard)", 175),
        ("gre_214", "gre_214", "gre_fiber", Some("5.230.119.214"), "GRE Fiber – öffentliche IP .214", 214),
        ("gre_215", "gre_215", "gre_fiber", Some("5.230.119.215"), "GRE Fiber – öffentliche IP .215", 215),
        ("vpnde", "vpnde", "vpnfra", None, "WireGuard Deutschland", 204),
        ("vpnus", "vpnus", "vpnusa", None, "WireGuard USA", 205),
        ("webgate", "webgate", "vpnagn", None, "Webgate VPN", 207),
        ("stargate", "stargate", "enp1s0.12", None, "Starlink-Uplink (via VLAN 12)", 208),
        ("buda", "buda", "buda", None, "Budapest Tunnel", 203),
        ("mobile", "mobile", "mobile", None, "WireGuard Roadwarrior", 209),
        ("ppp0", "main", "ppp0", None, "Direkt über DSL (ppp0)", 100),
    ];

    for (name, table_name, interface, src_ip, description, mark) in gateways {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO gateways (name, table_name, interface, src_ip, description, mark)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(name)
        .bind(table_name)
        .bind(interface)
        .bind(src_ip)
        .bind(description)
        .bind(mark)
        .execute(pool)
        .await?;
    }
    Ok(())
}

// --- Client operations ---

pub async fn upsert_client(
    pool: &SqlitePool,
    ip: &str,
    mac: Option<&str>,
    hostname: Option<&str>,
) -> Result<()> {
    let now = Utc::now().timestamp();
    sqlx::query(
        r#"
        INSERT INTO clients (ip, mac, hostname, gateway, first_seen, last_seen, active)
        VALUES (?1, ?2, ?3, 'gre_175', ?4, ?4, 1)
        ON CONFLICT(ip) DO UPDATE SET
            mac       = COALESCE(?2, mac),
            hostname  = COALESCE(?3, hostname),
            last_seen = ?4,
            active    = 1
        "#,
    )
    .bind(ip)
    .bind(mac)
    .bind(hostname)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_client_gateway(pool: &SqlitePool, ip: &str, gateway: &str) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE clients SET gateway = ?1 WHERE ip = ?2",
    )
    .bind(gateway)
    .bind(ip)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_client_label(pool: &SqlitePool, ip: &str, label: &str) -> Result<bool> {
    let result = sqlx::query("UPDATE clients SET label = ?1 WHERE ip = ?2")
        .bind(label)
        .bind(ip)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_clients(pool: &SqlitePool) -> Result<Vec<Client>> {
    let clients = sqlx::query_as::<_, Client>("SELECT * FROM clients ORDER BY ip")
        .fetch_all(pool)
        .await?;
    Ok(clients)
}

pub async fn get_client(pool: &SqlitePool, ip: &str) -> Result<Option<Client>> {
    let client = sqlx::query_as::<_, Client>("SELECT * FROM clients WHERE ip = ?1")
        .bind(ip)
        .fetch_optional(pool)
        .await?;
    Ok(client)
}

pub async fn list_active_clients(pool: &SqlitePool) -> Result<Vec<Client>> {
    let clients = sqlx::query_as::<_, Client>(
        "SELECT * FROM clients WHERE active = 1 ORDER BY ip",
    )
    .fetch_all(pool)
    .await?;
    Ok(clients)
}

// --- Gateway operations ---

pub async fn list_gateways(pool: &SqlitePool) -> Result<Vec<Gateway>> {
    let gateways = sqlx::query_as::<_, Gateway>("SELECT * FROM gateways ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(gateways)
}

pub async fn get_gateway(pool: &SqlitePool, name: &str) -> Result<Option<Gateway>> {
    let gw = sqlx::query_as::<_, Gateway>("SELECT * FROM gateways WHERE name = ?1")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(gw)
}

// --- Monitor events ---

pub async fn insert_monitor_event(
    pool: &SqlitePool,
    event: &str,
    detail: Option<&str>,
) -> Result<()> {
    let now = Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO monitor_events (ts, event, detail) VALUES (?1, ?2, ?3)",
    )
    .bind(now)
    .bind(event)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_recent_events(pool: &SqlitePool, limit: i64) -> Result<Vec<MonitorEvent>> {
    let events = sqlx::query_as::<_, MonitorEvent>(
        "SELECT * FROM monitor_events ORDER BY ts DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(events)
}
