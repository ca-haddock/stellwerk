use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub db: DbConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub tls: TlsConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub monitoring: MonitoringConfig,
    #[serde(default)]
    pub homeassistant: HomeAssistantConfig,
    #[serde(default)]
    pub influxdb: InfluxDbConfig,
    #[serde(default)]
    pub networks: NetworksConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub dns: DnsConfig,
    #[serde(default)]
    pub mullvad: Option<MullvadConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DbConfig {
    pub path: String,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            path: "/var/lib/stellwerk/stellwerk.db".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiConfig {
    pub listen: String,
    /// Optionaler zweiter HTTP-Listener (ohne TLS), z.B. für Kiosk-Zugang im LAN
    #[serde(default)]
    pub listen_http: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:1443".to_string(),
            listen_http: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert: String,
    pub key: String,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cert: "/etc/certs/tatooine.cc/fullchain.cer".to_string(),
            key: "/etc/certs/tatooine.cc/*.tatooine.cc.key".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub enabled: bool,
    pub username: String,
    /// SHA-256 hash des Passworts: echo -n "passwort" | sha256sum
    pub password_hash: String,
    /// Viewer-Account (read-only); leer = deaktiviert
    #[serde(default)]
    pub viewer_username: String,
    #[serde(default)]
    pub viewer_password_hash: String,
    /// Kiosk-Token für passwortlosen Autostart (z.B. Chromium Kiosk-Modus)
    #[serde(default)]
    pub kiosk_token: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            username: "admin".to_string(),
            password_hash: String::new(),
            viewer_username: String::new(),
            viewer_password_hash: String::new(),
            kiosk_token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MonitoringConfig {
    pub check_interval_secs: u64,
    pub ppp0_check_host: String,
    pub gre_check_host: String,
    /// GRE→ppp0 Failover: bei GRE-Ausfall Default-Route in GRE-Tabellen auf ppp0 umschalten
    #[serde(default)]
    pub gre_failover_enabled: bool,
    /// Interface das als GRE gilt (default: "gre_fiber")
    #[serde(default = "MonitoringConfig::default_gre_interface")]
    pub gre_interface: String,
    /// Nexthop-IP für GRE-Default-Route (default: "172.16.10.1")
    #[serde(default = "MonitoringConfig::default_gre_nexthop")]
    pub gre_nexthop: String,
}

impl MonitoringConfig {
    fn default_gre_interface() -> String { "gre_fiber".to_string() }
    fn default_gre_nexthop() -> String { "172.16.10.1".to_string() }
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 30,
            ppp0_check_host: "8.8.8.8".to_string(),
            gre_check_host: "1.1.1.1".to_string(),
            gre_failover_enabled: false,
            gre_interface: "gre_fiber".to_string(),
            gre_nexthop: "172.16.10.1".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HomeAssistantConfig {
    pub url: String,
    pub token: String,
    pub starlink_entity: String,
    pub enabled: bool,
}

impl Default for HomeAssistantConfig {
    fn default() -> Self {
        Self {
            url: "http://homeassistant.local:8123".to_string(),
            token: String::new(),
            starlink_entity: "switch.starlink_modem".to_string(),
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InfluxDbConfig {
    pub url: String,
    pub token: String,
    pub bucket: String,
    pub org: String,
    pub enabled: bool,
}

impl Default for InfluxDbConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8086".to_string(),
            token: String::new(),
            bucket: "network".to_string(),
            org: "home".to_string(),
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworksConfig {
    pub scan_subnets: Vec<String>,
    pub scan_interval_secs: u64,
}

impl Default for NetworksConfig {
    fn default() -> Self {
        Self {
            scan_subnets: vec![
                "172.16.0.0/22".to_string(),
                "172.16.4.0/24".to_string(),
                "172.16.5.0/24".to_string(),
                "172.16.8.0/24".to_string(),
                "172.16.9.0/24".to_string(),
                "172.16.10.0/24".to_string(),
                "172.16.11.0/24".to_string(),
            ],
            scan_interval_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DefaultsConfig {
    pub gateway: String,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            gateway: "gre_175".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DnsConfig {
    /// Gateway-Name für Unbound-Upstream-Queries (z.B. "vpnde").
    /// None = kein spezielles DNS-Routing (Router-IP wird bei DNS-Leak-Tests sichtbar).
    pub gateway: Option<String>,
    /// Linux-Username unter dem Unbound läuft (default: "unbound").
    #[serde(default = "DnsConfig::default_user")]
    pub unbound_user: String,
    /// DNS-Leak-Schutz: pro Gateway den DNS-Server festlegen.
    /// Überschreibt beim Start die DB-Werte.
    /// Beispiel: { vpnde = "1.1.1.1", vpnus = "1.0.0.1" }
    #[serde(default)]
    pub gateway_dns: HashMap<String, String>,
    /// Benannte DNS-Server für das UI (name → IP).
    /// Beispiel: { local = "172.16.3.254", google = "8.8.8.8" }
    #[serde(default)]
    pub servers: HashMap<String, String>,
}

impl DnsConfig {
    fn default_user() -> String {
        "unbound".to_string()
    }
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            gateway: None,
            unbound_user: "unbound".to_string(),
            gateway_dns: HashMap::new(),
            servers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MullvadConfig {
    /// Mullvad Account-Nummer (16 Ziffern)
    pub account: String,
}

impl Default for MullvadConfig {
    fn default() -> Self {
        Self {
            account: String::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| "Failed to parse config file")?;
        Ok(config)
    }

    pub fn default_with_path(db_path: &str) -> Self {
        let mut c = Config::default();
        c.db.path = db_path.to_string();
        c
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db: DbConfig::default(),
            api: ApiConfig::default(),
            tls: TlsConfig::default(),
            auth: AuthConfig::default(),
            monitoring: MonitoringConfig::default(),
            homeassistant: HomeAssistantConfig::default(),
            influxdb: InfluxDbConfig::default(),
            networks: NetworksConfig::default(),
            defaults: DefaultsConfig::default(),
            dns: DnsConfig::default(),
            mullvad: None,
        }
    }
}
