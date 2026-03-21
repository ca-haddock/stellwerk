mod api;
mod config;
mod db;
mod discovery;
mod homeassistant;
mod monitor;
mod nftables;
mod routing;
mod scripts;
mod traffic;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{interval, Duration};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "stellwerk", about = "Network gateway manager for LAN clients")]
struct Args {
    #[arg(short, long, default_value = "/etc/stellwerk/config.toml")]
    config: PathBuf,

    /// Run without applying nftables/iproute2 rules (dry-run / testing)
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("stellwerk=info".parse()?))
        .init();

    let args = Args::parse();

    // Load config
    let cfg = if args.config.exists() {
        info!("Loading config from {}", args.config.display());
        config::Config::load(&args.config)?
    } else {
        warn!("Config file not found at {}, using defaults", args.config.display());
        config::Config::default()
    };

    // Database
    info!("Opening database: {}", cfg.db.path);
    let pool = db::init_pool(&cfg.db.path).await?;

    // Status
    let status = monitor::new_status();

    // Notify channel for routing reapply
    let reapply_notify = Arc::new(Notify::new());

    // HomeAssistant client
    let ha_client = if cfg.homeassistant.enabled && !cfg.homeassistant.token.is_empty() {
        Some(homeassistant::HomeAssistantClient::new(&cfg.homeassistant))
    } else {
        None
    };

    // Shared app state for API
    let app_state = api::AppState {
        pool: pool.clone(),
        status: status.clone(),
        default_gw: cfg.defaults.gateway.clone(),
        scan_tx: reapply_notify.clone(),
    };

    // Initial routing apply from DB
    if !args.dry_run {
        apply_routing(&pool, &cfg.defaults.gateway).await;
    }

    // Spawn tasks
    let pool_mon = pool.clone();
    let mon_cfg = cfg.monitoring.clone();
    let status_mon = status.clone();
    tokio::spawn(async move {
        monitor::run_monitor_loop(pool_mon, mon_cfg, ha_client, status_mon).await;
    });

    let pool_disc = pool.clone();
    let disc_cfg = cfg.networks.clone();
    let disc_notify = reapply_notify.clone();
    let default_gw_disc = cfg.defaults.gateway.clone();
    let dry_run = args.dry_run;
    tokio::spawn(async move {
        run_discovery_loop(pool_disc, disc_cfg, disc_notify, default_gw_disc, dry_run).await;
    });

    let pool_traf = pool.clone();
    let influx_cfg = cfg.influxdb.clone();
    tokio::spawn(async move {
        traffic::run_traffic_loop(pool_traf, influx_cfg).await;
    });

    // Reapply routing when notified (gateway changes from API)
    let pool_reapply = pool.clone();
    let default_gw_reapply = cfg.defaults.gateway.clone();
    let notify_reapply = reapply_notify.clone();
    tokio::spawn(async move {
        loop {
            notify_reapply.notified().await;
            if !dry_run {
                apply_routing(&pool_reapply, &default_gw_reapply).await;
            }
        }
    });

    // HTTP API
    let router = api::build_router(app_state);
    let listener = tokio::net::TcpListener::bind(&cfg.api.listen).await?;
    info!("Stellwerk listening on http://{}", cfg.api.listen);

    axum::serve(listener, router).await?;

    Ok(())
}

/// Apply all routing rules from DB (nftables + iproute2 + write scripts)
async fn apply_routing(pool: &sqlx::SqlitePool, default_gw: &str) {
    let clients = match db::list_active_clients(pool).await {
        Ok(c) => c,
        Err(e) => { warn!("Failed to list clients: {}", e); return; }
    };
    let gateways = match db::list_gateways(pool).await {
        Ok(g) => g,
        Err(e) => { warn!("Failed to list gateways: {}", e); return; }
    };

    if let Err(e) = nftables::apply_all(&clients, &gateways, default_gw).await {
        warn!("nftables apply failed: {}", e);
    }
    if let Err(e) = routing::apply_all(&clients, &gateways, default_gw).await {
        warn!("iproute2 apply failed: {}", e);
    }
    if let Err(e) = scripts::write_all(&clients, &gateways, default_gw).await {
        warn!("Script write failed: {}", e);
    }
}

async fn run_discovery_loop(
    pool: sqlx::SqlitePool,
    cfg: config::NetworksConfig,
    _notify: Arc<Notify>,
    default_gw: String,
    dry_run: bool,
) {
    // Initial scan on startup (short delay)
    tokio::time::sleep(Duration::from_secs(5)).await;

    let mut ticker = interval(Duration::from_secs(cfg.scan_interval_secs));

    loop {
        info!("Starting network discovery...");
        match discovery::discover_all(&cfg.scan_subnets).await {
            Ok(hosts) => {
                info!("Discovered {} hosts", hosts.len());
                for host in &hosts {
                    if let Err(e) = db::upsert_client(
                        &pool,
                        &host.ip,
                        host.mac.as_deref(),
                        host.hostname.as_deref(),
                    ).await {
                        warn!("DB upsert error for {}: {}", host.ip, e);
                    }
                }
                // Also read ARP table for quick updates
                if let Ok(arp) = discovery::read_arp_table().await {
                    for host in arp {
                        let _ = db::upsert_client(&pool, &host.ip, host.mac.as_deref(), None).await;
                    }
                }
                if !dry_run {
                    apply_routing(&pool, &default_gw).await;
                }
            }
            Err(e) => warn!("Discovery error: {}", e),
        }

        ticker.tick().await;
    }
}
