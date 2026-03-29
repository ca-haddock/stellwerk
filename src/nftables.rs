use anyhow::Result;
use serde_json::Value;
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::DnsConfig;
use crate::db::{Client, Gateway, NetworkConfig};

const TABLE_NAME: &str = "stellwerk";
const RFC1918: &str = "{ 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 }";
/// fwmark für Unbound-DNS-Traffic (0x53 = 83, mnemonic: Port 53)
const DNS_MARK: &str = "0x53";

/// Rebuild the entire stellwerk nftables table from scratch
pub async fn apply_all(clients: &[Client], gateways: &[Gateway], networks: &[NetworkConfig], default_gw: &str, dns: &DnsConfig) -> Result<()> {
    let ruleset = build_ruleset(clients, gateways, networks, default_gw, dns);
    apply_ruleset(&ruleset).await
}

/// Build the nftables ruleset as a string.
///
/// Routing is handled by source-IP-based iproute2 rules (no fwmarks needed).
/// nftables is responsible for NAT, traffic accounting, and gateway-only isolation.
pub fn build_ruleset(clients: &[Client], gateways: &[Gateway], networks: &[NetworkConfig], _default_gw: &str, dns: &DnsConfig) -> String {
    let mut lines = Vec::new();

    lines.push(format!("table inet {} {{", TABLE_NAME));

    // Named counters must be declared before the chains that reference them
    for client in clients {
        if client.active == 0 {
            continue;
        }
        let cn = ip_to_counter_name(&client.ip);
        lines.push(format!("  counter {}_in_intern {{}}", cn));
        lines.push(format!("  counter {}_in_extern {{}}", cn));
        lines.push(format!("  counter {}_out_intern {{}}", cn));
        lines.push(format!("  counter {}_out_extern {{}}", cn));
    }

    // Postrouting: NAT
    // Clients routed through a gateway with a fixed src_ip get per-client SNAT.
    // All other clients get interface-level masquerade.
    lines.push("  chain postrouting {".to_string());
    lines.push("    type nat hook postrouting priority srcnat; policy accept;".to_string());

    // Unbound DNS-Traffic (fwmark 0x53) auf die src_ip des konfigurierten Gateways SNATen.
    // Ohne diese Regel würde Unbound via GRE mit der internen Tunnel-IP masqueradet,
    // die der GRE-Endpoint nicht ins Internet weiterleitet.
    if let Some(dns_gw_name) = &dns.gateway {
        if let Some(gw) = gateways.iter().find(|g| &g.name == dns_gw_name) {
            if let Some(src_ip) = &gw.src_ip {
                lines.push(format!("    meta mark {} snat to {};", DNS_MARK, src_ip));
            }
        }
    }

    for client in clients {
        if client.active == 0 {
            continue;
        }
        if let Some(gw) = gateways.iter().find(|g| g.name == client.gateway) {
            if let Some(src_ip) = &gw.src_ip {
                lines.push(format!(
                    "    ip saddr {} snat to {};",
                    client.ip, src_ip
                ));
            }
        }
    }

    // Interface-level masquerade for gateways without a fixed src_ip
    let mut seen_ifaces: std::collections::HashSet<String> = std::collections::HashSet::new();
    for gw in gateways {
        if gw.src_ip.is_some() {
            continue; // handled per-client above
        }
        if gw.interface == "lo" {
            continue; // masquerade on loopback breaks local DNS (unbound)
        }
        if seen_ifaces.contains(&gw.interface) {
            continue;
        }
        lines.push(format!("    oifname \"{}\" masquerade;", gw.interface));
        seen_ifaces.insert(gw.interface.clone());
    }

    lines.push("  }".to_string());

    // Outbound accounting: count bytes leaving per client, split intern/extern
    lines.push("  chain accounting_out {".to_string());
    lines.push("    type filter hook postrouting priority srcnat + 5; policy accept;".to_string());
    for client in clients {
        if client.active == 0 {
            continue;
        }
        let cn = ip_to_counter_name(&client.ip);
        lines.push(format!("    ip saddr {} ip daddr {} counter name {}_out_intern;", client.ip, RFC1918, cn));
        lines.push(format!("    ip saddr {} ip daddr != {} counter name {}_out_extern;", client.ip, RFC1918, cn));
    }
    lines.push("  }".to_string());

    // Inbound accounting: count bytes arriving per client, split intern/extern
    lines.push("  chain accounting_in {".to_string());
    lines.push("    type filter hook prerouting priority dstnat + 5; policy accept;".to_string());
    for client in clients {
        if client.active == 0 {
            continue;
        }
        let cn = ip_to_counter_name(&client.ip);
        lines.push(format!("    ip daddr {} ip saddr {} counter name {}_in_intern;", client.ip, RFC1918, cn));
        lines.push(format!("    ip daddr {} ip saddr != {} counter name {}_in_extern;", client.ip, RFC1918, cn));
    }
    lines.push("  }".to_string());

    // Forward chain: block inter-VLAN routing for "gateway_only" subnets.
    // Clients in these subnets can reach the internet but NOT other LAN subnets.
    let gw_only: Vec<&str> = networks.iter()
        .filter(|n| n.gateway_only != 0 && n.internal_only == 0)
        .map(|n| n.subnet.as_str())
        .collect();

    if !gw_only.is_empty() {
        lines.push("  chain forward {".to_string());
        lines.push("    type filter hook forward priority filter; policy accept;".to_string());
        for subnet in &gw_only {
            lines.push(format!(
                "    ip saddr {} ip daddr != {} ip daddr {{ 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 }} drop;",
                subnet, subnet
            ));
        }
        lines.push("  }".to_string());
    }

    // DNS-Leak-Schutz: Unbound-Traffic per fwmark auf konfigurierten Gateway routen.
    // Die ip rule (routing.rs) leitet mark=0x53 in die passende Gateway-Tabelle.
    if dns.gateway.is_some() {
        lines.push("  chain dns_output {".to_string());
        lines.push("    type route hook output priority mangle; policy accept;".to_string());
        // IPv6-Traffic von Unbound droppen: gre_fiber hat kein IPv6-Routing,
        // daher würden IPv6-DNS-Anfragen über ppp0 leaken. Stattdessen zwingt
        // das Drop Unbound auf IPv4-only und verhindert DNS-Leaks über ppp0.
        lines.push(format!("    meta skuid \"{}\" meta nfproto ipv6 drop;", dns.unbound_user));
        lines.push(format!("    meta skuid \"{}\" mark set {};", dns.unbound_user, DNS_MARK));
        lines.push("  }".to_string());
    }

    // Per-Client DNS-Leak-Schutz: DNAT port 53 für Clients mit dns_ip am Gateway.
    // Das Paket behält die Client-Source-IP → ip rule routet es automatisch
    // durch die richtige Gateway-Tabelle. Kein Leak möglich.
    let dns_rules: Vec<String> = clients.iter()
        .filter(|c| c.active != 0)
        .filter_map(|c| {
            let client_subnet = crate::discovery::ip_to_subnet_cidr(&c.ip);
            let dns_ip = c.dns_ip.as_deref()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    gateways.iter()
                        .find(|g| g.name == c.gateway)
                        .and_then(|g| g.dns_ip.as_deref().filter(|s| !s.is_empty()))
                })
                .or_else(|| {
                    client_subnet.as_deref().and_then(|sn| {
                        networks.iter()
                            .find(|n| n.subnet == sn)
                            .and_then(|n| n.dns_ip.as_deref().filter(|s| !s.is_empty()))
                    })
                })?;
            Some(format!(
                "    ip saddr {} udp dport 53 dnat to {};\n    ip saddr {} tcp dport 53 dnat to {};",
                c.ip, dns_ip, c.ip, dns_ip
            ))
        })
        .collect();

    if !dns_rules.is_empty() {
        lines.push("  chain prerouting_dns {".to_string());
        lines.push("    type nat hook prerouting priority dstnat; policy accept;".to_string());
        for rule in dns_rules {
            lines.push(rule);
        }
        lines.push("  }".to_string());
    }

    lines.push("}".to_string());
    lines.join("\n")
}

/// Replace the stellwerk nftables table atomically
pub async fn apply_ruleset(ruleset: &str) -> Result<()> {
    let _ = Command::new("nft")
        .args(["delete", "table", "inet", TABLE_NAME])
        .status()
        .await;

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
