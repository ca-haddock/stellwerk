use anyhow::Result;
use serde_json::Value;
use tokio::process::Command;
use tracing::{info, warn};

use crate::db::{Client, Gateway};

const TABLE_NAME: &str = "stellwerk";

/// Rebuild the entire stellwerk nftables table from scratch
pub async fn apply_all(clients: &[Client], gateways: &[Gateway], default_gw: &str) -> Result<()> {
    let ruleset = build_ruleset(clients, gateways, default_gw);
    apply_ruleset(&ruleset).await
}

/// Build the nftables ruleset as a string
pub fn build_ruleset(clients: &[Client], gateways: &[Gateway], default_gw: &str) -> String {
    let mut lines = Vec::new();

    lines.push(format!("table inet {} {{", TABLE_NAME));

    // Prerouting chain: mark packets per client source IP
    lines.push("  chain prerouting {".to_string());
    lines.push("    type filter hook prerouting priority mangle; policy accept;".to_string());

    for client in clients {
        if client.active == 0 || client.gateway == default_gw {
            continue;
        }
        if let Some(gw) = gateways.iter().find(|g| g.name == client.gateway) {
            lines.push(format!(
                "    ip saddr {} meta mark set {};  # gateway: {}",
                client.ip, gw.mark, gw.name
            ));
        }
    }

    lines.push("  }".to_string());

    // Postrouting: masquerade per gateway interface
    lines.push("  chain postrouting {".to_string());
    lines.push("    type nat hook postrouting priority srcnat; policy accept;".to_string());

    // For GRE with specific source IPs, use SNAT; others use masquerade
    let mut seen_interfaces: std::collections::HashSet<String> = std::collections::HashSet::new();
    for gw in gateways {
        if seen_interfaces.contains(&gw.interface) {
            continue;
        }
        if let Some(src_ip) = &gw.src_ip {
            lines.push(format!(
                "    oifname \"{}\" snat to {};",
                gw.interface, src_ip
            ));
        } else {
            lines.push(format!(
                "    oifname \"{}\" masquerade;",
                gw.interface
            ));
        }
        seen_interfaces.insert(gw.interface.clone());
    }

    lines.push("  }".to_string());

    // Accounting chains for traffic tracking (bytes in/out per client)
    lines.push("  chain accounting_out {".to_string());
    lines.push("    type filter hook postrouting priority srcnat + 5; policy accept;".to_string());
    for client in clients {
        if client.active == 0 {
            continue;
        }
        let counter_name = ip_to_counter_name(&client.ip);
        lines.push(format!(
            "    ip saddr {} counter name {}_out;",
            client.ip, counter_name
        ));
    }
    lines.push("  }".to_string());

    lines.push("  chain accounting_in {".to_string());
    lines.push("    type filter hook prerouting priority mangle + 5; policy accept;".to_string());
    for client in clients {
        if client.active == 0 {
            continue;
        }
        let counter_name = ip_to_counter_name(&client.ip);
        lines.push(format!(
            "    ip daddr {} counter name {}_in;",
            client.ip, counter_name
        ));
    }
    lines.push("  }".to_string());

    // Named counters (must be declared separately)
    for client in clients {
        if client.active == 0 {
            continue;
        }
        let counter_name = ip_to_counter_name(&client.ip);
        lines.push(format!("  counter {}_in {{}}", counter_name));
        lines.push(format!("  counter {}_out {{}}", counter_name));
    }

    lines.push("}".to_string());
    lines.join("\n")
}

/// Replace the stellwerk nftables table
pub async fn apply_ruleset(ruleset: &str) -> Result<()> {
    // Delete existing table if present
    let _ = Command::new("nft")
        .args(["delete", "table", "inet", TABLE_NAME])
        .status()
        .await;

    // Write to temp file and apply
    let tmp_path = "/tmp/stellwerk-nft.conf";
    tokio::fs::write(tmp_path, ruleset).await?;

    let output = Command::new("nft")
        .args(["-f", tmp_path])
        .output()
        .await?;

    if output.status.success() {
        info!("nftables ruleset applied successfully");
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        warn!("nft error: {}", err);
        return Err(anyhow::anyhow!("nft failed: {}", err));
    }
    Ok(())
}

/// Read traffic counters from nftables JSON output
pub async fn read_counters() -> Result<Vec<CounterValue>> {
    let output = Command::new("nft")
        .args(["-j", "list", "table", "inet", TABLE_NAME])
        .output()
        .await?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let json: Value = serde_json::from_slice(&output.stdout)?;
    let mut counters = Vec::new();

    if let Some(objects) = json["nftables"].as_array() {
        for obj in objects {
            if let Some(counter) = obj.get("counter") {
                let name = counter["name"].as_str().unwrap_or("").to_string();
                let bytes = counter["bytes"].as_u64().unwrap_or(0);
                let packets = counter["packets"].as_u64().unwrap_or(0);
                if !name.is_empty() {
                    counters.push(CounterValue { name, bytes, packets });
                }
            }
        }
    }
    Ok(counters)
}

#[derive(Debug, Clone)]
pub struct CounterValue {
    pub name: String,
    pub bytes: u64,
    pub packets: u64,
}

/// Convert IP to a valid nftables counter name (dots → underscores)
pub fn ip_to_counter_name(ip: &str) -> String {
    format!("c_{}", ip.replace('.', "_"))
}

/// Check if nft is available
pub async fn check_available() -> bool {
    Command::new("nft")
        .args(["--version"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
