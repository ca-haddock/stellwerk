# Stellwerk – Implementierungsplan

## Übersicht

Rust-Daemon für den Router (wall) der pro LAN-Client eine individuelle Gateway-Entscheidung ermöglicht.

## Netzwerk (Ist-Zustand)

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
| `vpnnig` | – | Nigeria WireGuard (inaktiv) |

## Gateways / Routing-Tables

| Name | rt_table | Interface | Mark | Beschreibung |
|---|---|---|---|---|
| gre_175 | gre_175 | gre_fiber | 175 | DEFAULT |
| gre_214 | gre_214 | gre_fiber | 214 | |
| gre_215 | gre_215 | gre_fiber | 215 | |
| vpnde | vpnde | vpnfra | 204 | |
| vpnus | vpnus | vpnusa | 205 | |
| webgate | webgate | vpnagn | 207 | |
| stargate | stargate | enp1s0.12 | 208 | Starlink |
| buda | buda | buda | 203 | |
| mobile | mobile | mobile | 209 | |
| ppp0 | main | ppp0 | 100 | |

## Routing-Mechanismus

```
nftables mangle prerouting:
  ip saddr <client_ip> meta mark set <mark>

ip rule:
  fwmark <mark> lookup <table_name> priority <1000+mark>
```

Default (kein mark): nutzt Hauptrouting → gre_175

## Persistenz

- Systemd service: stellwerk.service
  - ExecStartPre: /etc/stellwerk/failsafe.sh
  - ExecStart: /usr/local/bin/stellwerk
- Failsafe-Skript: /etc/stellwerk/failsafe.sh (SSH sichern)
- Apply-Skript: /etc/stellwerk/apply-routing.sh (generiert)
- nftables-Skript: /etc/stellwerk/nftables-stellwerk.nft (generiert)

## Monitoring / Failover

- Alle 30s: ping -I ppp0 8.8.8.8
- Bei Ausfall: HomeAssistant REST API → Starlink einschalten
- Ereignisse in SQLite (monitor_events)

## Traffic → InfluxDB

- Alle 60s: nft -j list table inet stellwerk → Counter lesen
- Delta berechnen → InfluxDB Line Protocol schreiben
- Tags: client IP, gateway

## Installation

```bash
cargo build --release
sudo install -m 755 target/release/stellwerk /usr/local/bin/
sudo mkdir -p /etc/stellwerk /var/lib/stellwerk
sudo cp config/default.toml /etc/stellwerk/config.toml
# Config anpassen (HA token, InfluxDB etc.)
sudo nano /etc/stellwerk/config.toml

# Systemd
sudo cp stellwerk.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now stellwerk

# Web-UI
http://172.16.8.254:8080
```
