mod api;
mod auth;
mod config;
mod db;
mod discovery;
mod homeassistant;
mod interfaces;
mod monitor;
mod mullvad;
mod nftables;
mod routing;
mod scripts;
mod traffic;

use anyhow::{Context, Result};
use clap::Parser;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{interval, Duration};
use tokio_rustls::rustls;
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
    let ha_client_api = ha_client.clone();

    // Sessions für Auth
    let sessions = auth::new_sessions();
    let viewer_sessions = auth::new_sessions();

    // DNS servers: aus config.toml [dns.servers], alphabetisch sortiert
    let mut dns_servers: Vec<(String, String)> = cfg.dns.servers.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    dns_servers.sort_by(|a, b| a.0.cmp(&b.0));

    // Shared app state for API
    let app_state = api::AppState {
        pool: pool.clone(),
        status: status.clone(),
        default_gw: cfg.defaults.gateway.clone(),
        scan_subnets: cfg.networks.scan_subnets.clone(),
        scan_tx: reapply_notify.clone(),
        sessions,
        viewer_sessions,
        auth_enabled: cfg.auth.enabled,
        username: cfg.auth.username.clone(),
        password_hash: cfg.auth.password_hash.clone(),
        viewer_username: cfg.auth.viewer_username.clone(),
        viewer_password_hash: cfg.auth.viewer_password_hash.clone(),
        kiosk_token: cfg.auth.kiosk_token.clone(),
        dns_servers,
        mullvad_config: cfg.mullvad.clone(),
        ha_client: ha_client_api,
    };

    // Initial routing apply from DB
    if !args.dry_run {
        apply_routing(&pool, &cfg.defaults.gateway, &cfg.dns.unbound_user).await;
    }

    // Spawn tasks
    let pool_mon = pool.clone();
    let mon_cfg = cfg.monitoring.clone();
    let status_mon = status.clone();
    let scan_tx_mon = reapply_notify.clone();
    tokio::spawn(async move {
        monitor::run_monitor_loop(pool_mon, mon_cfg, ha_client, status_mon, scan_tx_mon).await;
    });

    let pool_disc = pool.clone();
    let disc_cfg = cfg.networks.clone();
    let disc_notify = reapply_notify.clone();
    let default_gw_disc = cfg.defaults.gateway.clone();
    let unbound_user_disc = cfg.dns.unbound_user.clone();
    let dry_run = args.dry_run;
    tokio::spawn(async move {
        run_discovery_loop(pool_disc, disc_cfg, disc_notify, default_gw_disc, unbound_user_disc, dry_run).await;
    });

    let pool_traf = pool.clone();
    let influx_cfg = cfg.influxdb.clone();
    tokio::spawn(async move {
        traffic::run_traffic_loop(pool_traf, influx_cfg).await;
    });

    let influx_cfg_iface = cfg.influxdb.clone();
    tokio::spawn(async move {
        interfaces::run_interface_loop(influx_cfg_iface).await;
    });

    // Reapply routing when notified (gateway changes from API)
    let pool_reapply = pool.clone();
    let default_gw_reapply = cfg.defaults.gateway.clone();
    let unbound_user_reapply = cfg.dns.unbound_user.clone();
    let notify_reapply = reapply_notify.clone();
    tokio::spawn(async move {
        loop {
            notify_reapply.notified().await;
            if !dry_run {
                apply_routing(&pool_reapply, &default_gw_reapply, &unbound_user_reapply).await;
            }
        }
    });

    // HTTP(S) API
    let router = api::build_router(app_state);

    // Optionaler zweiter HTTP-Listener (ohne TLS), z.B. für Kiosk im LAN
    if let Some(http_addr) = &cfg.api.listen_http {
        let http_router = router.clone();
        let http_addr = http_addr.clone();
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(&http_addr).await {
                Ok(listener) => {
                    info!("Stellwerk HTTP (kiosk) listening on http://{}", http_addr);
                    let _ = axum::serve(listener, http_router).await;
                }
                Err(e) => warn!("HTTP kiosk listener konnte nicht gestartet werden ({}): {}", http_addr, e),
            }
        });
    }

    if cfg.tls.enabled {
        serve_tls(router, &cfg.tls, &cfg.api.listen).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(&cfg.api.listen).await?;
        info!("Stellwerk listening on http://{}", cfg.api.listen);
        axum::serve(listener, router).await?;
    }

    Ok(())
}

/// Apply all routing rules from DB (nftables + iproute2 + write scripts)
async fn apply_routing(pool: &sqlx::SqlitePool, default_gw: &str, unbound_user: &str) {
    // Lade unbound_gateway aus DB (überschreibt config.toml)
    let unbound_gateway = db::get_setting(pool, "unbound-gateway").await.ok().flatten();
    let dns = config::DnsConfig {
        gateway: unbound_gateway,
        unbound_user: unbound_user.to_string(),
        gateway_dns: std::collections::HashMap::new(),
        servers: std::collections::HashMap::new(),
    };

    let clients = match db::list_active_clients(pool).await {
        Ok(c) => c,
        Err(e) => { warn!("Failed to list clients: {}", e); return; }
    };
    let gateways = match db::list_gateways(pool).await {
        Ok(g) => g,
        Err(e) => { warn!("Failed to list gateways: {}", e); return; }
    };
    let networks = match db::list_networks(pool).await {
        Ok(n) => n,
        Err(e) => { warn!("Failed to list networks: {}", e); vec![] }
    };

    if let Err(e) = nftables::apply_all(&clients, &gateways, &networks, default_gw, &dns).await {
        warn!("nftables apply failed: {}", e);
    }
    if let Err(e) = routing::apply_all(&clients, &gateways, &networks, default_gw, &dns).await {
        warn!("iproute2 apply failed: {}", e);
    }
    if let Err(e) = scripts::write_all(&clients, &gateways, default_gw, &dns).await {
        warn!("Script write failed: {}", e);
    }
    if let Err(e) = scripts::configure_unbound(&dns, &gateways).await {
        warn!("Unbound config failed: {}", e);
    }
}

async fn run_discovery_loop(
    pool: sqlx::SqlitePool,
    cfg: config::NetworksConfig,
    _notify: Arc<Notify>,
    default_gw: String,
    unbound_user: String,
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
                // Map IPv6 addresses via NDP table (MAC → IPv6, display only)
                if let Ok(ndp) = discovery::read_ndp_table().await {
                    for (mac, ipv6) in &ndp {
                        let _ = db::update_ipv6_by_mac(&pool, mac, ipv6).await;
                    }
                }
                if !dry_run {
                    apply_routing(&pool, &default_gw, &unbound_user).await;
                }
            }
            Err(e) => warn!("Discovery error: {}", e),
        }

        ticker.tick().await;
    }
}

/// TLS-Server mit tokio-rustls + hyper
async fn serve_tls(app: axum::Router, tls_cfg: &config::TlsConfig, listen: &str) -> Result<()> {
    let certs = load_certs(&tls_cfg.cert)?;
    let key = load_private_key(&tls_cfg.key)?;

    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("TLS Konfiguration fehlgeschlagen")?;

    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(listen).await?;
    info!("Stellwerk listening on https://{}", listen);

    loop {
        let (stream, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => { warn!("TLS Handshake fehlgeschlagen: {}", e); return; }
            };
            let io = TokioIo::new(tls_stream);
            let svc = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                let mut app = app.clone();
                async move {
                    let req = req.map(axum::body::Body::new);
                    tower::Service::call(&mut app, req).await
                }
            });
            if let Err(e) = HyperBuilder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, svc)
                .await
            {
                warn!("Verbindungsfehler: {}", e);
            }
        });
    }
}

fn load_certs(path: &str) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let f = std::fs::File::open(path).with_context(|| format!("Zertifikat nicht gefunden: {}", path))?;
    rustls_pemfile::certs(&mut std::io::BufReader::new(f))
        .collect::<Result<Vec<_>, _>>()
        .context("Zertifikat konnte nicht geparst werden")
}

fn load_private_key(path: &str) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let f = std::fs::File::open(path).with_context(|| format!("Privater Schlüssel nicht gefunden: {}", path))?;
    rustls_pemfile::private_key(&mut std::io::BufReader::new(f))
        .context("Schlüssel konnte nicht gelesen werden")?
        .ok_or_else(|| anyhow::anyhow!("Kein privater Schlüssel in {}", path))
}
