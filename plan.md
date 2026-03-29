> **Historisches Dokument** – Ursprünglicher Implementierungsplan aus der Entwicklungsphase.
> Der beschriebene fwmark-basierte Routing-Mechanismus wurde durch source-IP-basiertes
> iproute2 Policy Routing ersetzt. Aktuelle Architektur: siehe [`ARCHITECTURE.md`](ARCHITECTURE.md).

---

# Stellwerk – Ursprünglicher Implementierungsplan

## Netzwerk (Ist-Zustand zum Zeitpunkt der Planung)

| Interface | IP | Zweck |
|---|---|---|
| `ppp0` | 92.116.141.237 | Haupt-Internet (DSL) |
| `gre_fiber` | 5.230.119.175/214/215 | GRE-Tunnel, öffentliche IPs |
| `vpnfra` / `vpnde` | 10.64.149.198 | WireGuard Deutschland |
| `vpnusa` / `vpnus` | 10.65.182.254 | WireGuard USA |
| `vpnagn` / `webgate` | 10.65.182.254 | Webgate |
| `enp1s0.12` / `stargate` | (Starlink wenn aktiv) | Starlink via VLAN 12 |
| `buda` | 172.16.153.1 | Budapest Tunnel |
| `mobile` | 172.17.100.1 | WireGuard Roadwarrior |

## Geplanter Routing-Mechanismus (veraltet – nicht mehr verwendet)

```
nftables mangle prerouting:
  ip saddr <client_ip> meta mark set <mark>

ip rule:
  fwmark <mark> lookup <table_name> priority <1000+mark>
```

> Ersetzt durch: source-IP-basierte `ip rule add from <ip> lookup <table>`.
> Aktueller Mechanismus: siehe ARCHITECTURE.md → routing.rs

## Monitoring / Failover (ursprünglich geplant)

- Alle 30s: ping -I ppp0 8.8.8.8
- Bei Ausfall: HomeAssistant REST API → Starlink einschalten

> Inzwischen erweitert um: GRE-Failover, per-Client Autofallback, Gruppen-Failover.
