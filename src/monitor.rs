use std::collections::HashMap;
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
    pub gateway_health: HashMap<String, bool>, // interface → up/down
    pub last_check: i64,
}

pub type StatusRef = Arc<tokio::sync::RwLock<InterfaceStatus>>;

pub fn new_status() -> StatusRef {
    Arc::new(tokio::sync::RwLock::new(InterfaceStatus {
        ppp0_up: true,
        gre_up: true,
        gateway_health: HashMap::new(),
        last_check: 0,
    }))
}

/// Run the monitoring loop
pub async fn run_monitor_loop(
    pool: sqlx::SqlitePool,
    cfg: MonitoringConfig,
    ha: Option<HomeAssistantClient>,
    status: StatusRef,
    scan_tx: Arc<tokio::sync::Notify>,
) {
    let mut ticker = interval(Duration::from_secs(cfg.check_interval_secs));
    let mut ppp0_was_up = true;
    let mut gre_was_up = true;
    // Tracks last known state per interface for autofallback gateways
    let mut iface_was_up: HashMap<String, bool> = HashMap::new();

    loop {
        ticker.tick().await;

        let ppp0_now = http_check_through_interface(&cfg.check_url, Some("ppp0")).await;
        let gre_now = http_check_through_interface(&cfg.check_url, Some(&cfg.gre_interface)).await;

        {
            let mut s = status.write().await;
            s.ppp0_up = ppp0_now;
            s.gre_up = gre_now;
            s.last_check = chrono::Utc::now().timestamp();
        }

        // ppp0 state changes
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

        // Global GRE failover (routing-table level, non-autofallback clients)
        if cfg.gre_failover_enabled {
            if gre_was_up && !gre_now {
                warn!("GRE went DOWN – switching client routing to ppp0 fallback");
                let _ = insert_monitor_event(&pool, "gre_failover_active", Some(&cfg.gre_interface)).await;
                gre_failover_activate(&pool, &cfg).await;
            } else if !gre_was_up && gre_now {
                info!("GRE came back UP – restoring GRE default routes");
                let _ = insert_monitor_event(&pool, "gre_failover_restored", Some(&cfg.gre_interface)).await;
                gre_failover_restore(&pool, &cfg).await;
            }
        }

        // Per-client autofallback: monitor interfaces used by autofallback clients
        check_autofallback_gateways(
            &pool, &cfg, gre_now, &mut iface_was_up, &scan_tx
        ).await;

        // Gruppen-Failover: Gateway-Interface pro Gruppe überwachen
        check_group_failover(&pool, &cfg, gre_now, &mut iface_was_up, &scan_tx).await;

        ppp0_was_up = ppp0_now;
        gre_was_up = gre_now;
    }
}

/// Checks all interfaces used by autofallback clients and switches them to ppp0 on failure.
async fn check_autofallback_gateways(
    pool: &sqlx::SqlitePool,
    cfg: &MonitoringConfig,
    gre_now: bool,
    iface_was_up: &mut HashMap<String, bool>,
    scan_tx: &Arc<tokio::sync::Notify>,
) {
    // Find all gateways that have autofallback clients (current or in fallback mode)
    let gateways = match sqlx::query_as::<_, crate::db::Gateway>(
        "SELECT DISTINCT g.* FROM gateways g
         JOIN clients c ON (c.gateway = g.name OR c.original_gateway = g.name)
         WHERE c.autofallback = 1 AND c.active = 1
           AND g.interface != 'ppp0' AND g.interface != 'lo'"
    ).fetch_all(pool).await {
        Ok(gws) => gws,
        Err(e) => { warn!("autofallback: DB query failed: {}", e); return; }
    };

    if gateways.is_empty() { return; }

    // Group by interface to avoid pinging the same interface multiple times
    let mut checked: HashMap<String, bool> = HashMap::new();

    for gw in &gateways {
        let iface = &gw.interface;
        if checked.contains_key(iface) { continue; }

        // For gre_fiber use the already-checked result; for others HTTP-check now
        let now_up = if iface == &cfg.gre_interface {
            gre_now
        } else {
            http_check_through_interface(&cfg.check_url, Some(iface)).await
        };
        checked.insert(iface.clone(), now_up);

        let was_up = *iface_was_up.get(iface).unwrap_or(&true);

        if was_up && !now_up {
            // Collect all gateway names on this interface
            let gw_names: Vec<String> = gateways.iter()
                .filter(|g| &g.interface == iface)
                .map(|g| g.name.clone())
                .collect();
            let gw_refs: Vec<&str> = gw_names.iter().map(|s| s.as_str()).collect();

            warn!("Autofallback: interface {} DOWN – switching clients to ppp0", iface);
            let _ = insert_monitor_event(pool, "autofallback_active",
                Some(&format!("{} ({})", gw_names.join(","), iface))).await;

            match crate::db::activate_fallback_for_clients(pool, &gw_refs).await {
                Ok(n) if n > 0 => {
                    info!("Autofallback: {} client(s) switched to ppp0", n);
                    scan_tx.notify_one();
                }
                Ok(_) => {}
                Err(e) => warn!("Autofallback activate failed: {}", e),
            }
        } else if !was_up && now_up {
            let gw_names: Vec<String> = gateways.iter()
                .filter(|g| &g.interface == iface)
                .map(|g| g.name.clone())
                .collect();
            let gw_refs: Vec<&str> = gw_names.iter().map(|s| s.as_str()).collect();

            info!("Autofallback: interface {} UP – restoring clients", iface);
            let _ = insert_monitor_event(pool, "autofallback_restored",
                Some(&format!("{} ({})", gw_names.join(","), iface))).await;

            match crate::db::restore_fallback_clients(pool, &gw_refs).await {
                Ok(n) if n > 0 => {
                    info!("Autofallback: {} client(s) restored to original gateway", n);
                    scan_tx.notify_one();
                }
                Ok(_) => {}
                Err(e) => warn!("Autofallback restore failed: {}", e),
            }
        }

        iface_was_up.insert(iface.clone(), now_up);
    }

    // Update status
}

/// Überwacht Gateway-Interfaces aller Gruppen mit fallback_gateway und schaltet bei Ausfall um.
async fn check_group_failover(
    pool: &sqlx::SqlitePool,
    cfg: &MonitoringConfig,
    gre_now: bool,
    iface_was_up: &mut HashMap<String, bool>,
    scan_tx: &Arc<tokio::sync::Notify>,
) {
    let groups = match crate::db::list_groups(pool).await {
        Ok(g) => g,
        Err(e) => { warn!("group_failover: DB query failed: {}", e); return; }
    };

    // Nur Gruppen mit konfiguriertem Fallback
    let groups: Vec<_> = groups.into_iter().filter(|g| g.fallback_gateway.is_some()).collect();
    if groups.is_empty() { return; }

    // Interface pro Gruppe aus der gateways-Tabelle laden
    let all_gateways = match sqlx::query_as::<_, crate::db::Gateway>("SELECT * FROM gateways")
        .fetch_all(pool).await {
        Ok(gws) => gws,
        Err(e) => { warn!("group_failover: gateways query failed: {}", e); return; }
    };

    let mut checked_ifaces: HashMap<String, bool> = HashMap::new();

    for group in &groups {
        let Some(gw) = all_gateways.iter().find(|g| g.name == group.gateway) else { continue; };
        let iface = &gw.interface;

        let now_up = if let Some(&cached) = checked_ifaces.get(iface) {
            cached
        } else {
            let up = if iface == &cfg.gre_interface {
                gre_now
            } else {
                http_check_through_interface(&cfg.check_url, Some(iface)).await
            };
            checked_ifaces.insert(iface.clone(), up);
            up
        };

        let was_up = *iface_was_up.get(&format!("grp:{}", group.name)).unwrap_or(&true);

        if was_up && !now_up && group.fallback_active == 0 {
            warn!("Gruppen-Failover: Gruppe '{}' Gateway {} DOWN → Fallback", group.name, group.gateway);
            let _ = insert_monitor_event(pool, "group_failover_active",
                Some(&format!("{} ({})", group.name, group.gateway))).await;
            if let Err(e) = crate::db::set_group_fallback(pool, &group.name, true).await {
                warn!("group_failover activate failed: {}", e);
            } else {
                scan_tx.notify_one();
            }
        } else if !was_up && now_up && group.fallback_active != 0 {
            info!("Gruppen-Failover: Gruppe '{}' Gateway {} UP → Wiederherstellen", group.name, group.gateway);
            let _ = insert_monitor_event(pool, "group_failover_restored",
                Some(&format!("{} ({})", group.name, group.gateway))).await;
            if let Err(e) = crate::db::set_group_fallback(pool, &group.name, false).await {
                warn!("group_failover restore failed: {}", e);
            } else {
                scan_tx.notify_one();
            }
        }

        iface_was_up.insert(format!("grp:{}", group.name), now_up);
    }
}

/// Schaltet Default-Route in allen GRE-Gateway-Tabellen auf ppp0 um
async fn gre_failover_activate(pool: &sqlx::SqlitePool, cfg: &MonitoringConfig) {
    let gateways = match sqlx::query_as::<_, crate::db::Gateway>("SELECT * FROM gateways")
        .fetch_all(pool)
        .await
    {
        Ok(gws) => gws,
        Err(e) => { warn!("gre_failover_activate: DB query failed: {}", e); return; }
    };

    let ppp0_nexthop = match get_ppp0_nexthop().await {
        Some(nh) => nh,
        None => { warn!("gre_failover_activate: kein ppp0-Nexthop gefunden"); return; }
    };

    for gw in gateways.iter().filter(|g| g.interface == cfg.gre_interface) {
        let ok = Command::new("ip")
            .args(["route", "replace", "default", "via", &ppp0_nexthop, "dev", "ppp0",
                   "table", &gw.table_name])
            .status().await
            .map(|s| s.success()).unwrap_or(false);
        if ok {
            info!("Failover: table {} → ppp0 via {}", gw.table_name, ppp0_nexthop);
        } else {
            warn!("Failover: table {} → ppp0 fehlgeschlagen", gw.table_name);
        }
    }
}

/// Stellt GRE-Default-Routen in allen GRE-Gateway-Tabellen wieder her
async fn gre_failover_restore(pool: &sqlx::SqlitePool, cfg: &MonitoringConfig) {
    let gateways = match sqlx::query_as::<_, crate::db::Gateway>("SELECT * FROM gateways")
        .fetch_all(pool)
        .await
    {
        Ok(gws) => gws,
        Err(e) => { warn!("gre_failover_restore: DB query failed: {}", e); return; }
    };

    for gw in gateways.iter().filter(|g| g.interface == cfg.gre_interface) {
        let mut args = vec![
            "route", "replace", "default",
            "via", &cfg.gre_nexthop,
            "dev", &cfg.gre_interface,
        ];
        let src_arg;
        if let Some(ref src_ip) = gw.src_ip {
            src_arg = src_ip.clone();
            args.extend_from_slice(&["src", &src_arg]);
        }
        args.extend_from_slice(&["table", &gw.table_name]);

        let ok = Command::new("ip")
            .args(&args)
            .status().await
            .map(|s| s.success()).unwrap_or(false);
        if ok {
            info!("Failover restored: table {} → GRE via {}", gw.table_name, cfg.gre_nexthop);
        } else {
            warn!("Failover restore: table {} fehlgeschlagen", gw.table_name);
        }
    }
}

/// Liest den aktuellen ppp0-Nexthop aus der main-Routing-Tabelle
async fn get_ppp0_nexthop() -> Option<String> {
    let output = Command::new("ip")
        .args(["route", "show", "table", "main"])
        .output().await.ok()?;

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|l| l.starts_with("default") && l.contains("ppp0"))
        .and_then(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            parts.windows(2)
                .find(|w| w[0] == "via")
                .map(|w| w[1].to_string())
        })
}

/// HTTP-Connectivity-Check durch ein bestimmtes Interface.
/// Nutzt curl --interface, da ICMP auf GRE/VPN-Interfaces oft geblockt ist.
async fn http_check_through_interface(url: &str, interface: Option<&str>) -> bool {
    let mut args = vec![
        "--max-time", "5",
        "--silent",
        "--fail",
        "--output", "/dev/null",
    ];
    if let Some(iface) = interface {
        args.push("--interface");
        args.push(iface);
    }
    args.push(url);

    Command::new("curl")
        .args(&args)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
