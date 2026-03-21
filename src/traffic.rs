use std::collections::HashMap;
use tokio::time::{interval, Duration};
use tracing::{debug, warn};

use crate::config::InfluxDbConfig;
use crate::nftables::{read_counters, ip_to_counter_name};
use crate::db::list_active_clients;

struct CounterState {
    bytes_in: u64,
    bytes_out: u64,
}

pub async fn run_traffic_loop(pool: sqlx::SqlitePool, cfg: InfluxDbConfig) {
    if !cfg.enabled {
        debug!("InfluxDB disabled, skipping traffic monitoring");
        return;
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("HTTP client");

    let mut ticker = interval(Duration::from_secs(60));
    let mut prev: HashMap<String, CounterState> = HashMap::new();

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

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let mut lines: Vec<String> = Vec::new();

        for client in &clients {
            let counter_base = ip_to_counter_name(&client.ip);
            let in_name = format!("{}_in", counter_base);
            let out_name = format!("{}_out", counter_base);

            let bytes_in = counters.iter()
                .find(|c| c.name == in_name)
                .map(|c| c.bytes)
                .unwrap_or(0);
            let bytes_out = counters.iter()
                .find(|c| c.name == out_name)
                .map(|c| c.bytes)
                .unwrap_or(0);

            // Calculate delta
            let (delta_in, delta_out) = if let Some(prev_state) = prev.get(&client.ip) {
                let di = bytes_in.saturating_sub(prev_state.bytes_in);
                let dout = bytes_out.saturating_sub(prev_state.bytes_out);
                (di, dout)
            } else {
                (0u64, 0u64)
            };

            prev.insert(client.ip.clone(), CounterState { bytes_in, bytes_out });

            // Build InfluxDB line protocol
            let ip_tag = client.ip.replace('.', "_");
            let gateway_tag = &client.gateway;
            let line = format!(
                "stellwerk_traffic,client={},gateway={} bytes_in={}i,bytes_out={}i {}",
                ip_tag, gateway_tag, delta_in, delta_out, now
            );
            lines.push(line);
        }

        if lines.is_empty() {
            continue;
        }

        // Write to InfluxDB
        let url = format!("{}/api/v2/write?org={}&bucket={}&precision=ns",
            cfg.url.trim_end_matches('/'), cfg.org, cfg.bucket);
        let body = lines.join("\n");

        let result = http
            .post(&url)
            .header("Authorization", format!("Token {}", cfg.token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(body)
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                debug!("Traffic data written to InfluxDB ({} series)", lines.len());
            }
            Ok(resp) => {
                warn!("InfluxDB write error: {}", resp.status());
            }
            Err(e) => {
                warn!("InfluxDB connection error: {}", e);
            }
        }
    }
}
