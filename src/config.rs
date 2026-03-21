use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub db: DbConfig,
    #[serde(default)]
    pub api: ApiConfig,
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
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MonitoringConfig {
    pub check_interval_secs: u64,
    pub ppp0_check_host: String,
    pub gre_check_host: String,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 30,
            ppp0_check_host: "8.8.8.8".to_string(),
            gre_check_host: "1.1.1.1".to_string(),
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
            monitoring: MonitoringConfig::default(),
            homeassistant: HomeAssistantConfig::default(),
            influxdb: InfluxDbConfig::default(),
            networks: NetworksConfig::default(),
            defaults: DefaultsConfig::default(),
        }
    }
}
