use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{info, warn};

const MULLVAD_RELAY_API: &str = "https://api.mullvad.net/public/relays/wireguard/v2/";
const MULLVAD_AUTH_API: &str = "https://api.mullvad.net/auth/v1/token";
const MULLVAD_KEYS_API: &str = "https://api.mullvad.net/app/v1/wireguard-keys";
const WG_STAGING_DIR: &str = "/home/stellwerk/wg";
const WG_CONFIG_DIR: &str = "/etc/wireguard";
const RT_TABLES_PATH: &str = "/etc/iproute2/rt_tables";
const MULLVAD_TABLE_BASE: u32 = 220;
const MULLVAD_MARK_BASE: i64 = 220;

// ── API response structs ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ApiResponse {
    locations: std::collections::HashMap<String, ApiLocation>,
    wireguard: ApiWireguard,
}

#[derive(Debug, Deserialize)]
struct ApiLocation {
    country: String,
}

#[derive(Debug, Deserialize)]
struct ApiWireguard {
    relays: Vec<ApiRelay>,
}

#[derive(Debug, Deserialize)]
struct ApiRelay {
    hostname: String,
    location: String,
    ipv4_addr_in: String,
    public_key: String,
    active: bool,
    weight: u32,
}

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MullvadCountry {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MullvadRelay {
    pub hostname: String,
    pub ipv4_addr_in: String,
    pub public_key: String,
    pub weight: u32,
}

// ── API queries ───────────────────────────────────────────────────────────────

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn fetch_api() -> Result<ApiResponse> {
    http_client()
        .get(MULLVAD_RELAY_API)
        .send()
        .await?
        .json::<ApiResponse>()
        .await
        .context("Mullvad Relay-API Antwort konnte nicht geparst werden")
}

// ── Key-Management ────────────────────────────────────────────────────────────

/// Generiert ein WireGuard-Keypair via `wg genkey` / `wg pubkey`.
/// Gibt (private_key, public_key) zurück.
pub async fn generate_keypair() -> Result<(String, String)> {
    let priv_out = Command::new("wg")
        .arg("genkey")
        .output()
        .await
        .context("wg genkey fehlgeschlagen — ist wireguard-tools installiert?")?;
    if !priv_out.status.success() {
        anyhow::bail!("wg genkey fehlgeschlagen");
    }
    let private_key = String::from_utf8_lossy(&priv_out.stdout).trim().to_string();

    let mut pub_cmd = Command::new("wg");
    pub_cmd.arg("pubkey");
    pub_cmd.stdin(std::process::Stdio::piped());
    pub_cmd.stdout(std::process::Stdio::piped());
    let mut child = pub_cmd.spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(private_key.as_bytes()).await?;
    }
    let pub_out = child.wait_with_output().await?;
    let public_key = String::from_utf8_lossy(&pub_out.stdout).trim().to_string();

    Ok((private_key, public_key))
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct KeyRegistrationResponse {
    ipv4_address: String,
}

/// Holt einen kurzlebigen Access Token von Mullvad via Account-Nummer.
async fn get_access_token(account: &str) -> Result<String> {
    let resp = http_client()
        .post(MULLVAD_AUTH_API)
        .json(&serde_json::json!({ "account_number": account }))
        .send()
        .await
        .context("Mullvad Auth fehlgeschlagen")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Mullvad Auth Fehler {}: {}", status, body);
    }

    let data: AuthResponse = resp.json().await
        .context("Mullvad Auth Antwort konnte nicht geparst werden")?;
    Ok(data.access_token)
}

/// Registriert einen WireGuard Public Key beim Mullvad-Account.
/// Gibt die zugewiesene IPv4-Adresse zurück (z.B. "10.64.138.42/32").
pub async fn register_key(account: &str, public_key: &str, device_name: Option<&str>) -> Result<String> {
    let token = get_access_token(account).await?;

    let mut body = serde_json::json!({ "pubkey": public_key });
    if let Some(name) = device_name {
        body["name"] = serde_json::Value::String(name.to_string());
    }

    let resp = http_client()
        .post(MULLVAD_KEYS_API)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
        .context("Mullvad Key-Registrierung fehlgeschlagen")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Mullvad API Fehler {}: {}", status, body);
    }

    let data: KeyRegistrationResponse = resp.json().await
        .context("Mullvad API Antwort konnte nicht geparst werden")?;
    Ok(data.ipv4_address)
}

/// Deregistriert einen WireGuard Public Key vom Mullvad-Account (best-effort).
pub async fn deregister_key(account: &str, public_key: &str) -> Result<()> {
    let token = get_access_token(account).await?;
    let url = format!("{}/{}", MULLVAD_KEYS_API, public_key);
    let resp = http_client()
        .delete(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .context("Mullvad Key-Deregistrierung fehlgeschlagen")?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!("Mullvad Key-Deregistrierung Fehler: {}", body);
    }
    Ok(())
}

pub async fn fetch_countries() -> Result<Vec<MullvadCountry>> {
    let resp = fetch_api().await?;
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (loc_key, loc_val) in &resp.locations {
        let cc = loc_key.split('-').next().unwrap_or("").to_string();
        if !cc.is_empty() {
            seen.entry(cc).or_insert_with(|| loc_val.country.clone());
        }
    }
    let mut countries: Vec<MullvadCountry> = seen
        .into_iter()
        .map(|(code, name)| MullvadCountry { code, name })
        .collect();
    countries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(countries)
}

pub async fn fetch_relays_for_country(country_code: &str) -> Result<Vec<MullvadRelay>> {
    let resp = fetch_api().await?;
    let prefix = format!("{}-", country_code);
    let mut relays: Vec<MullvadRelay> = resp
        .wireguard
        .relays
        .into_iter()
        .filter(|r| r.active && r.location.starts_with(&prefix))
        .map(|r| MullvadRelay {
            hostname: r.hostname,
            ipv4_addr_in: r.ipv4_addr_in,
            public_key: r.public_key,
            weight: r.weight,
        })
        .collect();
    // Highest weight first
    relays.sort_by(|a, b| b.weight.cmp(&a.weight));
    Ok(relays)
}

// ── Naming helpers ────────────────────────────────────────────────────────────

pub fn interface_name(country_code: &str) -> String {
    format!("mu{}", country_code)
}

pub fn is_mullvad_interface(name: &str) -> bool {
    name.starts_with("mu") && name.len() > 2 && name.chars().skip(2).all(|c| c.is_ascii_alphabetic())
}

// ── WireGuard config ──────────────────────────────────────────────────────────

pub fn generate_wg_config(private_key: &str, address: &str, relay: &MullvadRelay) -> String {
    format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}\nTable = off\n\n[Peer]\nPublicKey = {}\nEndpoint = {}:51820\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = 25\n",
        private_key, address, relay.public_key, relay.ipv4_addr_in
    )
}


pub async fn write_wg_config(country_code: &str, config: &str) -> Result<()> {
    // Staging-Kopie (stellwerk-owned, zur Inspektion/Backup)
    tokio::fs::create_dir_all(WG_STAGING_DIR).await.ok();
    let staging = format!("{}/mu{}.conf", WG_STAGING_DIR, country_code);
    tokio::fs::write(&staging, config).await.ok();
    let _ = Command::new("chmod").args(["600", &staging]).status().await;

    // Aktive Config nach /etc/wireguard/ (stellwerk hat ACL-Schreibrecht)
    let active = format!("{}/mu{}.conf", WG_CONFIG_DIR, country_code);
    tokio::fs::write(&active, config)
        .await
        .with_context(|| format!("Konnte WireGuard-Config nicht schreiben: {}", active))?;
    let _ = Command::new("chmod").args(["600", &active]).status().await;
    info!("WireGuard config geschrieben: {}", active);
    Ok(())
}

pub async fn remove_wg_config(country_code: &str) {
    let staging = format!("{}/mu{}.conf", WG_STAGING_DIR, country_code);
    tokio::fs::remove_file(&staging).await.ok();
    let active = format!("{}/mu{}.conf", WG_CONFIG_DIR, country_code);
    tokio::fs::remove_file(&active).await.ok();
}

// ── Routing table management ──────────────────────────────────────────────────

pub async fn next_free_table_number() -> u32 {
    let content = tokio::fs::read_to_string(RT_TABLES_PATH)
        .await
        .unwrap_or_default();
    let used: std::collections::HashSet<u32> = content
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') || l.is_empty() {
                return None;
            }
            l.split_whitespace().next()?.parse().ok()
        })
        .collect();
    (MULLVAD_TABLE_BASE..300)
        .find(|n| !used.contains(n))
        .unwrap_or(250)
}

pub async fn next_free_mark(pool: &sqlx::SqlitePool) -> i64 {
    let used: Vec<i64> = sqlx::query_scalar("SELECT mark FROM gateways")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let used_set: std::collections::HashSet<i64> = used.into_iter().collect();
    (MULLVAD_MARK_BASE..300i64)
        .find(|m| !used_set.contains(m))
        .unwrap_or(250)
}

/// Trägt ein beliebiges Interface in rt_tables ein.
pub async fn add_rt_table_entry_for(iface: &str, table_number: u32) -> Result<()> {
    let content = tokio::fs::read_to_string(RT_TABLES_PATH)
        .await
        .unwrap_or_default();
    let filtered: String = content
        .lines()
        .filter(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            parts.len() < 2 || parts[1] != iface
        })
        .map(|l| format!("{}\n", l))
        .collect();
    let new_content = format!("{}{}\t{}\n", filtered, table_number, iface);
    tokio::fs::write(RT_TABLES_PATH, new_content).await?;
    info!("rt_tables: {} {} hinzugefügt", table_number, iface);
    Ok(())
}

pub async fn add_rt_table_entry(country_code: &str, table_number: u32) -> Result<()> {
    let iface = format!("mu{}", country_code);
    let content = tokio::fs::read_to_string(RT_TABLES_PATH)
        .await
        .unwrap_or_default();
    // Remove existing entry for this interface if present
    let filtered: String = content
        .lines()
        .filter(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            parts.len() < 2 || parts[1] != iface
        })
        .map(|l| format!("{}\n", l))
        .collect();
    let new_content = format!("{}{}\t{}\n", filtered, table_number, iface);
    tokio::fs::write(RT_TABLES_PATH, new_content).await?;
    info!("rt_tables: {} {} hinzugefügt", table_number, iface);
    Ok(())
}

/// Entfernt ein beliebiges Interface aus rt_tables.
pub async fn remove_rt_table_entry_for(iface: &str) -> Result<()> {
    let content = tokio::fs::read_to_string(RT_TABLES_PATH)
        .await
        .unwrap_or_default();
    let filtered: String = content
        .lines()
        .filter(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            parts.len() < 2 || parts[1] != iface
        })
        .map(|l| format!("{}\n", l))
        .collect();
    tokio::fs::write(RT_TABLES_PATH, filtered).await?;
    info!("rt_tables: {} entfernt", iface);
    Ok(())
}

pub async fn remove_rt_table_entry(country_code: &str) -> Result<()> {
    remove_rt_table_entry_for(&format!("mu{}", country_code)).await
}

// ── Interface lifecycle ───────────────────────────────────────────────────────

pub async fn bring_up(country_code: &str) -> Result<()> {
    let conf = format!("{}/mu{}.conf", WG_CONFIG_DIR, country_code);
    // wg-quick erbt CAP_NET_ADMIN via AmbientCapabilities des stellwerk-Service
    let status = Command::new("wg-quick")
        .args(["up", &conf])
        .status()
        .await
        .context("wg-quick nicht gefunden — ist wireguard-tools installiert?")?;
    if !status.success() {
        anyhow::bail!("wg-quick up mu{} fehlgeschlagen", country_code);
    }
    info!("wg-quick up mu{}", country_code);
    Ok(())
}

pub async fn bring_down(country_code: &str) -> Result<()> {
    let conf = format!("{}/mu{}.conf", WG_CONFIG_DIR, country_code);
    let status = Command::new("wg-quick")
        .args(["down", &conf])
        .status()
        .await?;
    if !status.success() {
        warn!("wg-quick down mu{} fehlgeschlagen (Interface bereits down?)", country_code);
    }
    info!("wg-quick down mu{}", country_code);
    Ok(())
}

pub async fn is_up(country_code: &str) -> bool {
    let iface = interface_name(country_code);
    iface_exists(&iface).await
}

/// Prüft ob ein Interface existiert.
pub async fn iface_exists(iface: &str) -> bool {
    Command::new("ip")
        .args(["link", "show", iface])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Gibt alle aktiven WireGuard-Interfaces zurück (`wg show interfaces`).
pub async fn list_wg_interfaces() -> Vec<String> {
    let out = match Command::new("wg").args(["show", "interfaces"]).output().await {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// Default-Route für ein beliebiges Interface in seine Routing-Tabelle eintragen.
pub async fn add_default_route_for(iface: &str, table: &str) {
    let ok = Command::new("ip")
        .args(["route", "replace", "default", "dev", iface, "table", table])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        info!("Default-Route: default dev {} table {}", iface, table);
    } else {
        warn!("Default-Route {} fehlgeschlagen", iface);
    }
}

/// Nach wg-quick up: Default-Route in die Gateway-Tabelle eintragen.
pub async fn add_default_route(country_code: &str) {
    let iface = interface_name(country_code);
    add_default_route_for(&iface, &iface).await;
}
