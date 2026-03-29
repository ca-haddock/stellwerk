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
    pub group_name: Option<String>,
    pub gateway: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub active: i64,
    pub ipv6: Option<String>,
    pub dns_ip: Option<String>,
    pub autofallback: i64,
    pub original_gateway: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Gateway {
    pub name: String,
    pub table_name: String,
    pub interface: String,
    pub src_ip: Option<String>,
    pub description: Option<String>,
    pub mark: i64,
    pub dns_ip: Option<String>,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MullvadDevice {
    pub name: String,
    pub private_key: String,
    pub public_key: String,
    pub address: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MonitorEvent {
    pub id: i64,
    pub ts: i64,
    pub event: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NetworkConfig {
    pub subnet: String,
    pub default_gateway: String,
    pub internal_only: i64,
    pub gateway_only: i64,
    pub dns_ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Group {
    pub name: String,
    pub gateway: String,
    pub fallback_gateway: Option<String>,
    pub description: Option<String>,
    pub fallback_active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TrafficRecord {
    pub ip: String,
    pub gateway: String,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub bytes_in_intern: i64,
    pub bytes_out_intern: i64,
}

pub async fn init_pool(db_path: &str) -> Result<SqlitePool> {
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
            group_name TEXT,
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
        CREATE TABLE IF NOT EXISTS traffic (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            ip        TEXT NOT NULL,
            ts        INTEGER NOT NULL,
            bytes_in  INTEGER NOT NULL DEFAULT 0,
            bytes_out INTEGER NOT NULL DEFAULT 0,
            gateway   TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_clients_last_seen ON clients(last_seen);
        CREATE INDEX IF NOT EXISTS idx_monitor_events_ts ON monitor_events(ts DESC);
        CREATE INDEX IF NOT EXISTS idx_traffic_ip_ts ON traffic(ip, ts DESC);
        CREATE TABLE IF NOT EXISTS interface_meta (
            name    TEXT PRIMARY KEY,
            role    TEXT NOT NULL DEFAULT 'extern',
            enabled INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS networks (
            subnet          TEXT PRIMARY KEY,
            default_gateway TEXT NOT NULL DEFAULT 'gre_175',
            internal_only   INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS system_settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS mullvad_devices (
            name        TEXT PRIMARY KEY,
            private_key TEXT NOT NULL,
            public_key  TEXT NOT NULL,
            address     TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Idempotent column additions for existing databases
    sqlx::query("ALTER TABLE networks ADD COLUMN gateway_only INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE clients ADD COLUMN group_name TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE clients ADD COLUMN ipv6 TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE clients ADD COLUMN dns_ip TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE traffic ADD COLUMN bytes_in_intern INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE traffic ADD COLUMN bytes_out_intern INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE gateways ADD COLUMN dns_ip TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE networks ADD COLUMN dns_ip TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE gateways ADD COLUMN device_name TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE clients ADD COLUMN autofallback INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await
        .ok();
    sqlx::query("ALTER TABLE clients ADD COLUMN original_gateway TEXT")
        .execute(pool)
        .await
        .ok();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS groups (
            name             TEXT PRIMARY KEY,
            gateway          TEXT NOT NULL,
            fallback_gateway TEXT,
            description      TEXT,
            fallback_active  INTEGER NOT NULL DEFAULT 0
        )"
    )
    .execute(pool)
    .await
    .ok();

    Ok(())
}

async fn seed_gateways(pool: &SqlitePool) -> Result<()> {
    let gateways: Vec<(&str, &str, &str, Option<&str>, &str, i64)> = vec![
        ("gre_175", "gre_175", "gre_fiber", Some("5.230.119.175"), "GRE Fiber – öffentliche IP .175 (Standard)", 175),
        ("gre_214", "gre_214", "gre_fiber", Some("5.230.119.214"), "GRE Fiber – öffentliche IP .214", 214),
        ("gre_215", "gre_215", "gre_fiber", Some("5.230.119.215"), "GRE Fiber – öffentliche IP .215", 215),
        ("webgate", "webgate", "vpnagn", None, "Webgate VPN", 207),
        ("stargate", "stargate", "enp1s0.12", None, "Starlink-Uplink (via VLAN 12)", 208),
        ("buda", "buda", "buda", None, "Budapest Tunnel", 203),
        ("mobile", "mobile", "mobile", None, "WireGuard Roadwarrior", 209),
        ("ppp0", "main", "ppp0", None, "Direkt über DSL (ppp0)", 100),
        ("nointernet", "nointernet", "lo", None, "Kein Internet – nur LAN", 212),
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

        // Gateway-Interfaces immer als 'extern' markieren damit der Interfaces-Tab
        // sie nicht versehentlich als intern taggen kann und aus dem Dropdown filtert.
        if interface != "lo" {
            sqlx::query(
                "INSERT INTO interface_meta (name, role, enabled) VALUES (?1, 'extern', 1)
                 ON CONFLICT(name) DO UPDATE SET role = 'extern'"
            )
            .bind(interface)
            .execute(pool)
            .await?;
        }
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

/// Update the IPv6 address for all clients matching a given MAC address
pub async fn update_ipv6_by_mac(pool: &SqlitePool, mac: &str, ipv6: &str) -> Result<()> {
    sqlx::query("UPDATE clients SET ipv6 = ?1 WHERE mac = ?2")
        .bind(ipv6)
        .bind(mac)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_client_gateway(pool: &SqlitePool, ip: &str, gateway: &str) -> Result<bool> {
    let result = sqlx::query("UPDATE clients SET gateway = ?1 WHERE ip = ?2")
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

pub async fn set_client_group(pool: &SqlitePool, ip: &str, group_name: &str) -> Result<bool> {
    let val = if group_name.is_empty() { None } else { Some(group_name) };
    let result = sqlx::query("UPDATE clients SET group_name = ?1 WHERE ip = ?2")
        .bind(val)
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

pub async fn list_clients_filtered(
    pool: &SqlitePool,
    group: Option<&str>,
    gateway: Option<&str>,
) -> Result<Vec<Client>> {
    let clients = match (group, gateway) {
        (None, None) => {
            sqlx::query_as::<_, Client>("SELECT * FROM clients ORDER BY ip")
                .fetch_all(pool)
                .await?
        }
        (Some(g), None) => {
            sqlx::query_as::<_, Client>("SELECT * FROM clients WHERE group_name = ?1 ORDER BY ip")
                .bind(g)
                .fetch_all(pool)
                .await?
        }
        (None, Some(gw)) => {
            sqlx::query_as::<_, Client>("SELECT * FROM clients WHERE gateway = ?1 ORDER BY ip")
                .bind(gw)
                .fetch_all(pool)
                .await?
        }
        (Some(g), Some(gw)) => {
            sqlx::query_as::<_, Client>(
                "SELECT * FROM clients WHERE group_name = ?1 AND gateway = ?2 ORDER BY ip",
            )
            .bind(g)
            .bind(gw)
            .fetch_all(pool)
            .await?
        }
    };
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

pub async fn upsert_gateway(
    pool: &SqlitePool,
    name: &str,
    table_name: &str,
    interface: &str,
    src_ip: Option<&str>,
    description: &str,
    mark: i64,
    device_name: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO gateways (name, table_name, interface, src_ip, description, mark, device_name)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(name) DO UPDATE SET
            table_name  = ?2,
            interface   = ?3,
            src_ip      = ?4,
            description = ?5,
            mark        = ?6,
            device_name = ?7
        "#,
    )
    .bind(name)
    .bind(table_name)
    .bind(interface)
    .bind(src_ip)
    .bind(description)
    .bind(mark)
    .bind(device_name)
    .execute(pool)
    .await?;
    Ok(())
}

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

pub async fn set_gateway_dns_ip(pool: &SqlitePool, name: &str, dns_ip: Option<&str>) -> Result<bool> {
    let result = sqlx::query("UPDATE gateways SET dns_ip = ?1 WHERE name = ?2")
        .bind(dns_ip)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_client_autofallback(pool: &SqlitePool, ip: &str, enabled: bool) -> Result<bool> {
    let result = sqlx::query("UPDATE clients SET autofallback = ?1 WHERE ip = ?2")
        .bind(enabled as i64)
        .bind(ip)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Switches autofallback clients on the given gateways to ppp0.
/// Only affects clients that are not already in fallback mode (original_gateway IS NULL).
pub async fn activate_fallback_for_clients(pool: &SqlitePool, gw_names: &[&str]) -> Result<u64> {
    let mut total = 0u64;
    for gw in gw_names {
        let result = sqlx::query(
            "UPDATE clients SET original_gateway = gateway, gateway = 'ppp0'
             WHERE autofallback = 1 AND original_gateway IS NULL AND gateway = ?1"
        )
        .bind(gw)
        .execute(pool)
        .await?;
        total += result.rows_affected();
    }
    Ok(total)
}

/// Restores clients that were switched to ppp0 via autofallback back to their original gateway.
pub async fn restore_fallback_clients(pool: &SqlitePool, gw_names: &[&str]) -> Result<u64> {
    let mut total = 0u64;
    for gw in gw_names {
        let result = sqlx::query(
            "UPDATE clients SET gateway = original_gateway, original_gateway = NULL
             WHERE original_gateway = ?1"
        )
        .bind(gw)
        .execute(pool)
        .await?;
        total += result.rows_affected();
    }
    Ok(total)
}

pub async fn set_client_dns_ip(pool: &SqlitePool, ip: &str, dns_ip: Option<&str>) -> Result<bool> {
    let result = sqlx::query("UPDATE clients SET dns_ip = ?1 WHERE ip = ?2")
        .bind(dns_ip)
        .bind(ip)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM system_settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(v,)| v).filter(|v| !v.is_empty()))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2"
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

// --- Traffic accounting ---

pub async fn insert_traffic(
    pool: &SqlitePool,
    ip: &str,
    bytes_in: i64,
    bytes_out: i64,
    bytes_in_intern: i64,
    bytes_out_intern: i64,
    gateway: &str,
) -> Result<()> {
    let now = Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO traffic (ip, ts, bytes_in, bytes_out, bytes_in_intern, bytes_out_intern, gateway) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(ip)
    .bind(now)
    .bind(bytes_in)
    .bind(bytes_out)
    .bind(bytes_in_intern)
    .bind(bytes_out_intern)
    .bind(gateway)
    .execute(pool)
    .await?;
    Ok(())
}

/// Sum of traffic per client over the last 24 hours
pub async fn get_traffic_24h(pool: &SqlitePool) -> Result<Vec<TrafficRecord>> {
    let since = Utc::now().timestamp() - 86400;
    let records = sqlx::query_as::<_, TrafficRecord>(
        r#"
        SELECT ip, gateway,
               SUM(bytes_in) as bytes_in, SUM(bytes_out) as bytes_out,
               SUM(bytes_in_intern) as bytes_in_intern, SUM(bytes_out_intern) as bytes_out_intern
        FROM traffic
        WHERE ts > ?1
        GROUP BY ip
        ORDER BY (bytes_in + bytes_out) DESC
        "#,
    )
    .bind(since)
    .fetch_all(pool)
    .await?;
    Ok(records)
}

/// Delete traffic records older than N days to keep the DB lean
pub async fn cleanup_old_traffic(pool: &SqlitePool, days: i64) -> Result<u64> {
    let cutoff = Utc::now().timestamp() - days * 86400;
    let result = sqlx::query("DELETE FROM traffic WHERE ts < ?1")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

// --- Interface metadata ---

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct InterfaceMeta {
    pub name: String,
    pub role: String,    // "intern" | "extern"
    pub enabled: i64,
}

pub async fn list_interface_meta(pool: &SqlitePool) -> Result<Vec<InterfaceMeta>> {
    let meta = sqlx::query_as::<_, InterfaceMeta>("SELECT * FROM interface_meta ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(meta)
}

pub async fn upsert_interface_meta(
    pool: &SqlitePool,
    name: &str,
    role: &str,
    enabled: bool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO interface_meta (name, role, enabled)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(name) DO UPDATE SET role = ?2, enabled = ?3
        "#,
    )
    .bind(name)
    .bind(role)
    .bind(enabled as i64)
    .execute(pool)
    .await?;
    Ok(())
}

// --- Network configs ---

pub async fn list_networks(pool: &SqlitePool) -> Result<Vec<NetworkConfig>> {
    let nets = sqlx::query_as::<_, NetworkConfig>("SELECT * FROM networks ORDER BY subnet")
        .fetch_all(pool)
        .await?;
    Ok(nets)
}

pub async fn upsert_network(
    pool: &SqlitePool,
    subnet: &str,
    default_gateway: &str,
    internal_only: bool,
    gateway_only: bool,
    dns_ip: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO networks (subnet, default_gateway, internal_only, gateway_only, dns_ip)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(subnet) DO UPDATE SET
            default_gateway = ?2,
            internal_only   = ?3,
            gateway_only    = ?4,
            dns_ip          = ?5
        "#,
    )
    .bind(subnet)
    .bind(default_gateway)
    .bind(internal_only as i64)
    .bind(gateway_only as i64)
    .bind(dns_ip)
    .execute(pool)
    .await?;
    Ok(())
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

// --- Mullvad devices ---

pub async fn insert_mullvad_device(
    pool: &SqlitePool,
    name: &str,
    private_key: &str,
    public_key: &str,
    address: &str,
) -> Result<()> {
    let now = Utc::now().timestamp();
    sqlx::query(
        "INSERT OR REPLACE INTO mullvad_devices (name, private_key, public_key, address, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(name)
    .bind(private_key)
    .bind(public_key)
    .bind(address)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_mullvad_devices(pool: &SqlitePool) -> Result<Vec<MullvadDevice>> {
    let devices = sqlx::query_as::<_, MullvadDevice>(
        "SELECT * FROM mullvad_devices ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(devices)
}

pub async fn get_mullvad_device(pool: &SqlitePool, name: &str) -> Result<Option<MullvadDevice>> {
    let device = sqlx::query_as::<_, MullvadDevice>(
        "SELECT * FROM mullvad_devices WHERE name = ?1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(device)
}

pub async fn delete_mullvad_device(pool: &SqlitePool, name: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM mullvad_devices WHERE name = ?1")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

// --- Groups ---

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<Group>> {
    let groups = sqlx::query_as::<_, Group>(
        "SELECT name, gateway, fallback_gateway, description, fallback_active FROM groups ORDER BY name"
    )
    .fetch_all(pool)
    .await?;
    Ok(groups)
}

pub async fn upsert_group(pool: &SqlitePool, name: &str, gateway: &str, fallback_gateway: Option<&str>, description: Option<&str>) -> Result<()> {
    sqlx::query(
        "INSERT INTO groups (name, gateway, fallback_gateway, description, fallback_active)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(name) DO UPDATE SET gateway = ?2, fallback_gateway = ?3, description = ?4"
    )
    .bind(name)
    .bind(gateway)
    .bind(fallback_gateway)
    .bind(description)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_group(pool: &SqlitePool, name: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM groups WHERE name = ?1")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Schreibt den Gateway einer Gruppe auf alle Clients in der Gruppe.
pub async fn apply_group_gateway(pool: &SqlitePool, group_name: &str, gateway: &str) -> Result<u64> {
    let result = sqlx::query("UPDATE clients SET gateway = ?1 WHERE group_name = ?2")
        .bind(gateway)
        .bind(group_name)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Setzt fallback_active für eine Gruppe und schaltet alle Clients auf den entsprechenden Gateway.
pub async fn set_group_fallback(pool: &SqlitePool, group_name: &str, active: bool) -> Result<()> {
    let group = sqlx::query_as::<_, Group>(
        "SELECT name, gateway, fallback_gateway, description, fallback_active FROM groups WHERE name = ?1"
    )
    .bind(group_name)
    .fetch_optional(pool)
    .await?;

    let Some(g) = group else { return Ok(()); };

    let target_gw = if active {
        g.fallback_gateway.as_deref().unwrap_or(&g.gateway).to_string()
    } else {
        g.gateway.clone()
    };

    sqlx::query("UPDATE clients SET gateway = ?1 WHERE group_name = ?2")
        .bind(&target_gw)
        .bind(group_name)
        .execute(pool)
        .await?;

    sqlx::query("UPDATE groups SET fallback_active = ?1 WHERE name = ?2")
        .bind(active as i64)
        .bind(group_name)
        .execute(pool)
        .await?;

    Ok(())
}
