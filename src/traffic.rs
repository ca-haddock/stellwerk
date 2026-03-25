use std::collections::HashMap;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::config::InfluxDbConfig;
use crate::db::{self, list_active_clients};
use crate::nftables::{read_counters, ip_to_counter_name};

struct CounterState {
    bytes_in_intern: u64,
    bytes_in_extern: u64,
    bytes_out_intern: u64,
    bytes_out_extern: u64,
}

pub async fn run_traffic_loop(pool: sqlx::SqlitePool, cfg: InfluxDbConfig) {
    let http = if cfg.enabled {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .ok()
    } else {
        None
    };

    let mut ticker = interval(Duration::from_secs(60));
    let mut prev: HashMap<String, CounterState> = HashMap::new();
    let mut cleanup_counter = 0u32;

    loop {
        ticker.tick().await;

        let counters = match read_counters().await {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read nft counters: {}", e);
                continue;
            }
        };

        let clients = match list_active_clients(&pool).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to list clients: {}", e);
                continue;
            }
        };

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let mut influx_lines: Vec<String> = Vec::new();
        let mut stored = 0u32;

        for client in &clients {
            let cn = ip_to_counter_name(&client.ip);

            let bytes_in_intern = counters.iter().find(|c| c.name == format!("{}_in_intern", cn)).map(|c| c.bytes).unwrap_or(0);
            let bytes_in_extern = counters.iter().find(|c| c.name == format!("{}_in_extern", cn)).map(|c| c.bytes).unwrap_or(0);
            let bytes_out_intern = counters.iter().find(|c| c.name == format!("{}_out_intern", cn)).map(|c| c.bytes).unwrap_or(0);
            let bytes_out_extern = counters.iter().find(|c| c.name == format!("{}_out_extern", cn)).map(|c| c.bytes).unwrap_or(0);

            let (di_intern, di_extern, do_intern, do_extern) = if let Some(p) = prev.get(&client.ip) {
                (
                    bytes_in_intern.saturating_sub(p.bytes_in_intern),
                    bytes_in_extern.saturating_sub(p.bytes_in_extern),
                    bytes_out_intern.saturating_sub(p.bytes_out_intern),
                    bytes_out_extern.saturating_sub(p.bytes_out_extern),
                )
            } else {
                // First measurement – don't store yet (could be a large accumulated value)
                prev.insert(client.ip.clone(), CounterState { bytes_in_intern, bytes_in_extern, bytes_out_intern, bytes_out_extern });
                continue;
            };

            prev.insert(client.ip.clone(), CounterState { bytes_in_intern, bytes_in_extern, bytes_out_intern, bytes_out_extern });

            let delta_in = di_intern + di_extern;
            let delta_out = do_intern + do_extern;

            // Skip zero-delta entries to keep DB lean
            if delta_in == 0 && delta_out == 0 {
                continue;
            }

            // Store in SQLite
            if let Err(e) = db::insert_traffic(
                &pool,
                &client.ip,
                delta_in as i64,
                delta_out as i64,
                di_intern as i64,
                do_intern as i64,
                &client.gateway,
            ).await {
                warn!("Failed to store traffic for {}: {}", client.ip, e);
            } else {
                stored += 1;
            }

            // Build InfluxDB line protocol (optional)
            if http.is_some() {
                let ip_tag = client.ip.replace('.', "_");
                influx_lines.push(format!(
                    "stellwerk_traffic,client={},gateway={} bytes_in={}i,bytes_out={}i,bytes_in_intern={}i,bytes_out_intern={}i {}",
                    ip_tag, &client.gateway, delta_in, delta_out, di_intern, do_intern, now_ns
                ));
            }
        }

        if stored > 0 {
            debug!("Traffic: stored {} deltas to SQLite", stored);
        }

        // Cleanup old traffic data weekly (every ~10080 minutes = 168 ticks)
        cleanup_counter += 1;
        if cleanup_counter >= 168 {
            cleanup_counter = 0;
            match db::cleanup_old_traffic(&pool, 30).await {
                Ok(n) => info!("Traffic cleanup: removed {} old records (>30d)", n),
                Err(e) => warn!("Traffic cleanup error: {}", e),
            }
        }

        // Push to InfluxDB if configured
        if let Some(ref http_client) = http {
            if influx_lines.is_empty() {
                continue;
            }
            let url = format!(
                "{}/api/v2/write?org={}&bucket={}&precision=ns",
                cfg.url.trim_end_matches('/'), cfg.org, cfg.bucket
            );
            let body = influx_lines.join("\n");
            match http_client
                .post(&url)
                .header("Authorization", format!("Token {}", cfg.token))
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(body)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    debug!("Traffic: {} series pushed to InfluxDB", influx_lines.len());
                }
                Ok(resp) => warn!("InfluxDB write error: {}", resp.status()),
                Err(e) => warn!("InfluxDB connection error: {}", e),
            }
        }
    }
}
