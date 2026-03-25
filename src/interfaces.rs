use std::collections::HashMap;
use tokio::time::{interval, Duration};
use tracing::{debug, warn};

use crate::config::InfluxDbConfig;

#[derive(Clone)]
struct IfaceCounters {
    rx_bytes: u64,
    tx_bytes: u64,
    rx_packets: u64,
    tx_packets: u64,
    rx_errors: u64,
    tx_errors: u64,
    rx_drops: u64,
    tx_drops: u64,
}

/// Read interface stats from /proc/net/dev
fn read_proc_net_dev() -> HashMap<String, IfaceCounters> {
    let content = match std::fs::read_to_string("/proc/net/dev") {
        Ok(c) => c,
        Err(e) => { warn!("Kann /proc/net/dev nicht lesen: {}", e); return HashMap::new(); }
    };

    let mut result = HashMap::new();

    for line in content.lines().skip(2) {
        // Format: "  eth0: rx_bytes rx_pkts rx_err rx_drop ... tx_bytes tx_pkts tx_err tx_drop ..."
        let (name_part, stats_part) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        let name = name_part.trim().to_string();
        if name == "lo" {
            continue;
        }

        let nums: Vec<u64> = stats_part
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();

        if nums.len() < 16 {
            continue;
        }

        // /proc/net/dev columns:
        // rx: bytes(0) packets(1) errs(2) drop(3) fifo(4) frame(5) compressed(6) multicast(7)
        // tx: bytes(8) packets(9) errs(10) drop(11) fifo(12) colls(13) carrier(14) compressed(15)
        result.insert(name, IfaceCounters {
            rx_bytes:   nums[0],
            rx_packets: nums[1],
            rx_errors:  nums[2],
            rx_drops:   nums[3],
            tx_bytes:   nums[8],
            tx_packets: nums[9],
            tx_errors:  nums[10],
            tx_drops:   nums[11],
        });
    }

    result
}

pub async fn run_interface_loop(cfg: InfluxDbConfig) {
    if !cfg.enabled {
        return;
    }

    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => { warn!("HTTP-Client Fehler: {}", e); return; }
    };

    let url = format!(
        "{}/api/v2/write?org={}&bucket={}&precision=ns",
        cfg.url.trim_end_matches('/'), cfg.org, cfg.bucket
    );

    let mut ticker = interval(Duration::from_secs(30));
    let mut prev: HashMap<String, IfaceCounters> = HashMap::new();

    loop {
        ticker.tick().await;

        let current = read_proc_net_dev();
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let mut lines: Vec<String> = Vec::new();

        for (iface, cur) in &current {
            if let Some(p) = prev.get(iface) {
                let rx_bytes   = cur.rx_bytes.saturating_sub(p.rx_bytes);
                let tx_bytes   = cur.tx_bytes.saturating_sub(p.tx_bytes);
                let rx_packets = cur.rx_packets.saturating_sub(p.rx_packets);
                let tx_packets = cur.tx_packets.saturating_sub(p.tx_packets);
                let rx_errors  = cur.rx_errors.saturating_sub(p.rx_errors);
                let tx_errors  = cur.tx_errors.saturating_sub(p.tx_errors);
                let rx_drops   = cur.rx_drops.saturating_sub(p.rx_drops);
                let tx_drops   = cur.tx_drops.saturating_sub(p.tx_drops);

                lines.push(format!(
                    "stellwerk_interfaces,iface={} \
                     rx_bytes={}i,tx_bytes={}i,\
                     rx_packets={}i,tx_packets={}i,\
                     rx_errors={}i,tx_errors={}i,\
                     rx_drops={}i,tx_drops={}i \
                     {}",
                    iface,
                    rx_bytes, tx_bytes,
                    rx_packets, tx_packets,
                    rx_errors, tx_errors,
                    rx_drops, tx_drops,
                    now_ns
                ));
            }
        }

        prev = current;

        if lines.is_empty() {
            continue;
        }

        debug!("Interface-Stats: {} Interfaces → InfluxDB", lines.len());

        match http.post(&url)
            .header("Authorization", format!("Token {}", cfg.token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(lines.join("\n"))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => warn!("InfluxDB interface write error: {}", r.status()),
            Err(e) => warn!("InfluxDB interface connection error: {}", e),
        }
    }
}
