use anyhow::Result;
use tokio::process::Command;
use tracing::{info, warn};

use crate::db::{Client, Gateway};

/// Apply all routing rules from the database.
/// Clears existing stellwerk rules and re-adds them.
pub async fn apply_all(clients: &[Client], gateways: &[Gateway], default_gw: &str) -> Result<()> {
    // Remove old stellwerk marks (priority 1000-1999)
    flush_stellwerk_rules().await?;

    // Add rules for non-default clients
    for client in clients {
        if client.active == 0 {
            continue;
        }
        if client.gateway == default_gw {
            continue;
        }
        if let Some(gw) = gateways.iter().find(|g| g.name == client.gateway) {
            add_rule_for_client(&client.ip, gw).await?;
        }
    }
    Ok(())
}

/// Add a single ip rule + nftables mark for a client
pub async fn add_rule_for_client(ip: &str, gw: &Gateway) -> Result<()> {
    let priority = 1000 + gw.mark as u32;

    // ip rule: fwmark → routing table
    let status = Command::new("ip")
        .args(["rule", "add", "fwmark", &gw.mark.to_string(),
               "lookup", &gw.table_name, "priority", &priority.to_string()])
        .status()
        .await?;

    if status.success() {
        info!("ip rule add fwmark {} lookup {} for {}", gw.mark, gw.table_name, ip);
    } else {
        // Rule might already exist – not fatal
        warn!("ip rule add fwmark {} lookup {} possibly already exists ({})", gw.mark, gw.table_name, ip);
    }
    Ok(())
}

/// Delete ip rule for a specific mark
pub async fn del_rule_for_mark(mark: i64, table_name: &str) -> Result<()> {
    let priority = 1000 + mark as u32;
    let _ = Command::new("ip")
        .args(["rule", "del", "fwmark", &mark.to_string(),
               "lookup", table_name, "priority", &priority.to_string()])
        .status()
        .await?;
    Ok(())
}

/// Remove all ip rules in priority range 1000-1999 (our managed range)
pub async fn flush_stellwerk_rules() -> Result<()> {
    // List all rules, filter our priority range, delete them
    let output = Command::new("ip")
        .args(["rule", "show"])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        // Format: "1175:	from all fwmark 0xaf lookup gre_175"
        if let Some(prio_str) = line.split(':').next() {
            if let Ok(prio) = prio_str.trim().parse::<u32>() {
                if prio >= 1000 && prio < 2000 {
                    let _ = Command::new("ip")
                        .args(["rule", "del", "priority", &prio.to_string()])
                        .status()
                        .await;
                }
            }
        }
    }
    Ok(())
}

/// Get current ip rules as string (for scripts/debug)
pub async fn dump_rules() -> Result<String> {
    let output = Command::new("ip")
        .args(["rule", "show"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
