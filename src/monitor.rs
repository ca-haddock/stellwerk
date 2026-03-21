use std::sync::Arc;
use tokio::process::Command;
use tokio::time::{interval, Duration};
use tracing::{info, warn};

use crate::config::MonitoringConfig;
use crate::db::insert_monitor_event;
use crate::homeassistant::HomeAssistantClient;

#[derive(Debug, Clone, Default)]
pub struct InterfaceStatus {
    pub ppp0_up: bool,
    pub gre_up: bool,
    pub last_check: i64,
}

pub type StatusRef = Arc<tokio::sync::RwLock<InterfaceStatus>>;

pub fn new_status() -> StatusRef {
    Arc::new(tokio::sync::RwLock::new(InterfaceStatus {
        ppp0_up: true,
        gre_up: true,
        last_check: 0,
    }))
}

/// Run the monitoring loop
pub async fn run_monitor_loop(
    pool: sqlx::SqlitePool,
    cfg: MonitoringConfig,
    ha: Option<HomeAssistantClient>,
    status: StatusRef,
) {
    let mut ticker = interval(Duration::from_secs(cfg.check_interval_secs));
    let mut ppp0_was_up = true;

    loop {
        ticker.tick().await;

        let ppp0_now = ping_through_interface(&cfg.ppp0_check_host, Some("ppp0")).await;
        let gre_now = ping_through_interface(&cfg.gre_check_host, None).await;

        {
            let mut s = status.write().await;
            s.ppp0_up = ppp0_now;
            s.gre_up = gre_now;
            s.last_check = chrono::Utc::now().timestamp();
        }

        // Detect ppp0 state changes
        if ppp0_was_up && !ppp0_now {
            warn!("ppp0 went DOWN – notifying HomeAssistant to enable Starlink");
            let _ = insert_monitor_event(&pool, "ppp0_down", Some(&cfg.ppp0_check_host)).await;

            if let Some(ref ha_client) = ha {
                if let Err(e) = ha_client.turn_on_starlink().await {
                    warn!("Failed to enable Starlink via HA: {}", e);
                } else {
                    info!("Starlink enabled via HomeAssistant");
                    let _ = insert_monitor_event(&pool, "starlink_on", None).await;
                }
            }
        } else if !ppp0_was_up && ppp0_now {
            info!("ppp0 came back UP");
            let _ = insert_monitor_event(&pool, "ppp0_up", Some(&cfg.ppp0_check_host)).await;
        }

        ppp0_was_up = ppp0_now;
    }
}

/// Send one ping through a specific interface (or default routing)
async fn ping_through_interface(host: &str, interface: Option<&str>) -> bool {
    let mut args = vec!["-c", "1", "-W", "3", "-q"];
    if let Some(iface) = interface {
        args.push("-I");
        args.push(iface);
    }
    args.push(host);

    let result = Command::new("ping")
        .args(&args)
        .output()
        .await;

    match result {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}
