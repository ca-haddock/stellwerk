use anyhow::Result;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use tokio::process::Command;
use tracing::{debug, info};

/// Read the NDP (IPv6 neighbour) table and return MAC → IPv6 address mapping.
/// Only returns global/unique-local addresses (skips fe80:: link-local).
pub async fn read_ndp_table() -> Result<HashMap<String, String>> {
    let output = Command::new("ip")
        .args(["-6", "neigh", "show"])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut mac_to_ipv6: HashMap<String, String> = HashMap::new();

    for line in stdout.lines() {
        // Format: "2a02:8108:... dev enp3s0 lladdr 3c:61:05:d1:6e:ec STALE"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let ip6 = parts[0];
        // Skip link-local (fe80::) — not useful for display
        if ip6.starts_with("fe80") {
            continue;
        }
        // Skip FAILED/INCOMPLETE
        let lladdr_idx = match parts.iter().position(|&p| p == "lladdr") {
            Some(i) => i,
            None => continue,
        };
        let mac = parts[lladdr_idx + 1].to_lowercase();
        // Prefer keeping the first (most stable) address per MAC
        mac_to_ipv6.entry(mac).or_insert_with(|| ip6.to_string());
    }

    debug!("NDP table: {} IPv6 addresses mapped", mac_to_ipv6.len());
    Ok(mac_to_ipv6)
}

#[derive(Debug, Clone)]
pub struct DiscoveredHost {
    pub ip: String,
    pub mac: Option<String>,
    pub hostname: Option<String>,
}

/// Read the kernel ARP/neighbour table
pub async fn read_arp_table() -> Result<Vec<DiscoveredHost>> {
    let output = Command::new("ip")
        .args(["neigh", "show"])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut hosts = Vec::new();

    for line in stdout.lines() {
        // Format: "172.16.1.65 dev enp3s0 lladdr 3c:61:05:d1:6e:ec STALE"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let ip = parts[0];
        // Only IPv4
        if ip.parse::<Ipv4Addr>().is_err() {
            continue;
        }
        // Skip FAILED and INCOMPLETE entries (no lladdr)
        let lladdr_idx = parts.iter().position(|&p| p == "lladdr");
        let mac = lladdr_idx.map(|i| parts[i + 1].to_string());
        if mac.is_none() {
            continue; // skip FAILED/INCOMPLETE
        }

        hosts.push(DiscoveredHost {
            ip: ip.to_string(),
            mac,
            hostname: None,
        });
    }

    info!("ARP table: {} hosts found", hosts.len());
    Ok(hosts)
}

/// Ping sweep a subnet to populate the ARP table, then read it
/// Uses fping for fast parallel ping if available, else falls back to ping
pub async fn ping_sweep(subnet: &str) -> Result<Vec<DiscoveredHost>> {
    debug!("Ping sweep: {}", subnet);

    // Try fping first
    let fping_result = Command::new("fping")
        .args(["-a", "-q", "-g", subnet])
        .output()
        .await;

    if let Ok(output) = fping_result {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let ips: Vec<String> = stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        debug!("fping found {} alive hosts in {}", ips.len(), subnet);
    } else {
        // Fallback: skip active sweep, rely on ARP table only
        debug!("fping not available, relying on ARP table for {}", subnet);
    }

    // After ping sweep, ARP table is populated
    read_arp_table().await
}

/// Resolve hostname via reverse DNS (best-effort)
pub async fn resolve_hostname(ip: &str) -> Option<String> {
    let output = Command::new("getent")
        .args(["hosts", ip])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hostname = stdout.split_whitespace().nth(1)?;
    // Ignore "localhost" style results
    if hostname == ip {
        return None;
    }
    Some(hostname.to_string())
}

/// Full discovery: sweep all configured subnets, deduplicate, resolve names
pub async fn discover_all(subnets: &[String]) -> Result<Vec<DiscoveredHost>> {
    let mut seen: HashMap<String, DiscoveredHost> = HashMap::new();

    for subnet in subnets {
        match ping_sweep(subnet).await {
            Ok(hosts) => {
                for host in hosts {
                    seen.entry(host.ip.clone()).or_insert(host);
                }
            }
            Err(e) => {
                debug!("Sweep error for {}: {}", subnet, e);
            }
        }
    }

    // Try to resolve hostnames for discovered hosts
    let mut hosts: Vec<DiscoveredHost> = seen.into_values().collect();
    for host in &mut hosts {
        host.hostname = resolve_hostname(&host.ip).await;
    }

    hosts.sort_by(|a, b| {
        let a_ip = a.ip.parse::<Ipv4Addr>().unwrap_or(Ipv4Addr::UNSPECIFIED);
        let b_ip = b.ip.parse::<Ipv4Addr>().unwrap_or(Ipv4Addr::UNSPECIFIED);
        a_ip.cmp(&b_ip)
    });

    info!("Discovery complete: {} unique hosts", hosts.len());
    Ok(hosts)
}

/// Leitet aus einer Client-IP das /24-Subnetz ab (z.B. "172.16.1.5" → Some("172.16.1.0/24"))
pub fn ip_to_subnet_cidr(ip: &str) -> Option<String> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() == 4 {
        Some(format!("{}.{}.{}.0/24", parts[0], parts[1], parts[2]))
    } else {
        None
    }
}
