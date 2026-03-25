use anyhow::Result;
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::DnsConfig;
use crate::db::{Client, Gateway, NetworkConfig};

/// Get all local interface IPs (IPv4 only, excluding loopback)
async fn local_ips() -> Vec<String> {
    let output = Command::new("ip")
        .args(["addr", "show"])
        .output()
        .await
        .unwrap_or_else(|_| std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: vec![],
            stderr: vec![],
        });

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("inet ") {
                return None;
            }
            let ip_cidr = line.split_whitespace().nth(1)?;
            let ip = ip_cidr.split('/').next()?;
            if ip == "127.0.0.1" {
                return None;
            }
            Some(ip.to_string())
        })
        .collect()
}

/// Apply all routing rules from the database.
/// Uses source-IP-based policy routing (ip rule from <ip> lookup <table>).
pub async fn apply_all(clients: &[Client], gateways: &[Gateway], networks: &[NetworkConfig], default_gw: &str, dns: &DnsConfig) -> Result<()> {
    flush_stellwerk_rules().await?;

    // DNS-Leak-Schutz: Unbound-Traffic (fwmark 0x53) über konfigurierten Gateway routen.
    // Priorität 50 greift vor allen anderen Stellwerk-Regeln (999, 1000, 1999).
    // Sowohl IPv4 als auch IPv6 Regeln werden gesetzt.
    if let Some(dns_gw_name) = &dns.gateway {
        if let Some(gw) = gateways.iter().find(|g| &g.name == dns_gw_name) {
            for family in [&[][..], &["-6"][..]] {
                let status = Command::new("ip")
                    .args(family)
                    .args(["rule", "add", "fwmark", "0x53",
                           "lookup", &gw.table_name, "priority", "50"])
                    .status()
                    .await?;
                let proto = if family.is_empty() { "IPv4" } else { "IPv6" };
                if status.success() {
                    info!("ip {} rule add fwmark 0x53 → table {} (DNS via {})", proto, gw.table_name, gw.name);
                } else {
                    warn!("ip {} rule add fwmark 0x53 → {} possibly already exists", proto, gw.table_name);
                }
            }
        } else {
            warn!("DNS-Gateway '{}' nicht in Gateways gefunden – DNS-Routing übersprungen", dns_gw_name);
        }
    }

    // Per-client source-based rules.
    // Immer prio 1000 für alle aktiven Clients setzen, auch für Default-Gateway-Clients.
    // Ohne diese Regel würden Default-Gateway-Clients durch die Subnetz-Regel (prio 1500)
    // auf ein anderes Gateway geroutet — aber SNAT/DNS orientiert sich am DB-Gateway →
    // falsche Source-IP beim Egress → ISP droppt das Paket.
    for client in clients {
        if client.active == 0 {
            continue;
        }
        if let Some(gw) = gateways.iter().find(|g| g.name == client.gateway) {
            add_rule_for_client(&client.ip, gw).await?;
        }
    }

    // Protect the router's own traffic: all local IPs must use the main table,
    // otherwise the fallback rule below would misroute the server's own packets
    // through the GRE table (which has no LAN routes → router unreachable).
    for ip in local_ips().await {
        let status = Command::new("ip")
            .args(["rule", "add", "from", &format!("{}/32", ip),
                   "lookup", "main", "priority", "999"])
            .status()
            .await?;
        if status.success() {
            info!("ip rule add local protection: {} → main", ip);
        }
    }

    // Fallback: LAN client traffic without a specific rule → default gateway table.
    // Priority 999 rules above ensure the server's own traffic is not affected.
    if let Some(gw) = gateways.iter().find(|g| g.name == default_gw) {
        let status = Command::new("ip")
            .args(["rule", "add", "from", "172.16.0.0/12",
                   "lookup", &gw.table_name, "priority", "1999"])
            .status()
            .await?;
        if status.success() {
            info!("ip rule add fallback: 172.16.0.0/12 → table {}", gw.table_name);
        }
    }

    // Subnet-level rules (prio 1500) — override fallback but not per-client rules
    for net in networks {
        let table: String = if net.internal_only != 0 {
            "nointernet".to_string()
        } else {
            match gateways.iter().find(|g| g.name == net.default_gateway) {
                Some(gw) => gw.table_name.clone(),
                None => { warn!("Subnet {}: unknown gateway {}", net.subnet, net.default_gateway); continue; }
            }
        };
        let status = Command::new("ip")
            .args(["rule", "add", "from", &net.subnet, "lookup", &table, "priority", "1500"])
            .status()
            .await?;
        if status.success() {
            info!("ip rule add subnet {} → table {} (prio 1500)", net.subnet, table);
        }
    }

    // Copy LAN routes into every gateway table so clients can reach other subnets.
    // For Mullvad (mu<cc>) interfaces: also add the default route since Table=off
    // in wg-quick doesn't create routing table entries automatically.
    for gw in gateways {
        if gw.table_name == "main" {
            continue;
        }
        if let Err(e) = copy_lan_routes_to_table(&gw.table_name).await {
            warn!("copy_lan_routes → {}: {}", gw.table_name, e);
        }
        if crate::mullvad::is_mullvad_interface(&gw.interface) {
            crate::mullvad::add_default_route(gw.interface.trim_start_matches("mu")).await;
        }
    }

    // nointernet: LAN routes are copied above, but default must be blackhole
    // (no internet route → traffic to outside gets silently dropped)
    let _ = Command::new("ip")
        .args(["route", "replace", "blackhole", "default", "table", "nointernet"])
        .status()
        .await;
    info!("nointernet: blackhole default gesetzt");

    Ok(())
}

/// Copy all private LAN routes from the main table into a gateway table.
/// Without this, clients using that table can't reach other LAN subnets —
/// the table only has a default route (via GRE/VPN) but no local routes.
/// Errors (e.g. route already exists) are silently ignored.
async fn copy_lan_routes_to_table(table: &str) -> Result<()> {
    let output = Command::new("ip")
        .args(["route", "show"])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut count = 0usize;

    for line in stdout.lines() {
        if line.starts_with("default") {
            continue;
        }
        if !is_private_route(line) {
            continue;
        }
        let mut args: Vec<&str> = line.split_whitespace().collect();
        args.insert(0, "add");
        args.insert(0, "route");
        args.extend_from_slice(&["table", table]);
        let _ = Command::new("ip").args(&args).status().await;
        count += 1;
    }

    info!("LAN-Routen aus main → table {}: {} Routen", table, count);
    Ok(())
}

fn is_private_route(route_line: &str) -> bool {
    let first = route_line.split_whitespace().next().unwrap_or("");
    if first.starts_with("10.") || first.starts_with("192.168.") {
        return true;
    }
    if let Some(rest) = first.strip_prefix("172.") {
        let octet: u8 = rest.split('.').next()
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        return (16..=31).contains(&octet);
    }
    false
}

/// Add a source-IP-based policy rule for a single client
pub async fn add_rule_for_client(ip: &str, gw: &Gateway) -> Result<()> {
    let status = Command::new("ip")
        .args(["rule", "add", "from", ip,
               "lookup", &gw.table_name, "priority", "1000"])
        .status()
        .await?;

    if status.success() {
        info!("ip rule add from {} lookup {} ({})", ip, gw.table_name, gw.name);
    } else {
        warn!("ip rule add from {} lookup {} possibly already exists", ip, gw.table_name);
    }
    Ok(())
}

/// Remove all ip rules in our managed priority range (50–1999), both IPv4 and IPv6
pub async fn flush_stellwerk_rules() -> Result<()> {
    for family in [&[][..], &["-6"][..]] {
        let output = Command::new("ip")
            .args(family)
            .args(["rule", "show"])
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(prio_str) = line.split(':').next() {
                if let Ok(prio) = prio_str.trim().parse::<u32>() {
                    if prio >= 50 && prio < 2000 {
                        let _ = Command::new("ip")
                            .args(family)
                            .args(["rule", "del", "priority", &prio.to_string()])
                            .status()
                            .await;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Dump current ip rules (for debugging)
pub async fn dump_rules() -> Result<String> {
    let output = Command::new("ip")
        .args(["rule", "show"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
