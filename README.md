# Stellwerk

Ein Linux-Router-Daemon in Rust. Steuert den **Egress-Gateway pro LAN-Client** über iproute2 Policy Routing und nftables – mit Web-UI, REST-API, Mullvad-VPN-Integration und automatischem Failover.

## Features

- **Per-Client Gateway-Steuerung** – Jeder Client kann einem anderen Uplink zugewiesen werden (DSL, LTE, Starlink, Mullvad-VPN, eigene WireGuard-Tunnel)
- **Gruppen & Failover** – Clients in Gruppen zusammenfassen, gemeinsam umschalten, automatisches HA-Failover auf Fallback-Gateway
- **Mullvad VPN** – WireGuard-Keypairs direkt über die Mullvad-API registrieren, Länder verbinden/trennen
- **DNS-Leak-Schutz** – Unbound-Upstream-Traffic per fwmark über definierten Gateway; per-Client/Gateway/Subnetz DNS-Override per DNAT
- **Traffic-Accounting** – Per-Client-Zähler (intern/extern) via nftables, SQLite, optional InfluxDB
- **Automatisches Failover** – Uplink-Monitoring per Ping: ppp0-Failover (Starlink via HomeAssistant), GRE-Failover, per-Client-Autofallback, Gruppen-Failover
- **Web-UI** – Dark-Mode Single-Page-App mit admin/viewer/kiosk-Rollen
- **Recovery-Skripte** – Bei jedem Apply auto-generiert: `failsafe.sh`, `apply-routing.sh`, `nftables-stellwerk.nft`, `wg-helper.sh`

## Voraussetzungen

- Linux mit `iproute2`, `nftables`, `fping`, `wg-quick` (für Mullvad/WireGuard)
- Rust-Toolchain (stable)
- `unbound` als lokaler DNS-Resolver (optional)
- Routing-Tabelle `nointernet` (212) in `/etc/iproute2/rt_tables`
- Sudoers-Eintrag für `wg-quick` (Mullvad): `stellwerk ALL=(root) NOPASSWD: /usr/bin/systemctl start wg-quick@*, /usr/bin/systemctl stop wg-quick@*`

## Installation

```bash
# Erstinstallation (legt User an, setzt Capabilities, startet Service)
sudo ./install.sh

# Binary bauen + Service neu starten (inkrementiert Patch-Version automatisch)
sudo ./bin_install.sh

# Sicherer Start mit Konnektivitätstest und Auto-Rollback
sudo ./test_start.sh
```

## Build

```bash
cargo build --release
cargo clippy
```

## Konfiguration

Standardpfad: `/home/stellwerk/config.toml`
Vorlage: [`config/default.toml`](config/default.toml)

```toml
[api]
listen_https = "0.0.0.0:8443"
listen_http  = "0.0.0.0:8080"   # optional, für Kiosk-Modus (kein TLS)

[auth]
enabled       = true
password_hash = "..."            # echo -n "passwort" | sha256sum | awk '{print $1}'
viewer_password_hash = "..."
kiosk_token   = "geheimestoken"

[dns]
gateway      = "vpnde"           # Unbound-Upstream über diesen Gateway
unbound_user = "unbound"

[dns.gateway_dns]                # DNS-Server pro Gateway (beim Start in DB geschrieben)
vpnde = "1.1.1.1"
vpnus = "1.0.0.1"

[dns.servers]                    # Benannte DNS-Server für UI-Dropdowns
local      = "172.16.3.254"
cloudflare = "1.1.1.1"

[mullvad]
account = "1234567890123456"     # 16-stellige Mullvad-Kontonummer

[homeassistant]
enabled        = true
url            = "http://homeassistant.local:8123"
token          = "..."
starlink_entity = "switch.starlink"
```

## Service

```bash
systemctl enable stellwerk      # Autostart aktivieren
systemctl status stellwerk      # Status prüfen
journalctl -u stellwerk -f      # Logs verfolgen
```

## Entwicklung

```bash
# Ohne nftables/iproute2-Änderungen starten (sicher zum Testen)
stellwerk --dry-run

# Alternativer Config-Pfad
stellwerk --config /home/stellwerk/config.toml

# Log-Level
RUST_LOG=stellwerk=debug stellwerk
```

## Architektur

```
iproute2 Policy Routing
  prio 50    DNS-Leak-Schutz (fwmark 0x53 → DNS-Gateway)
  prio 999   Router-eigene IPs → main table
  prio 1000  Per-Client-Regeln
  prio 1500  Per-Subnetz-Regeln
  prio 1999  Fallback 172.16.0.0/12 → default_gw

nftables (inet stellwerk)
  postrouting   SNAT/Masquerade pro Gateway
  accounting_in  Eingehend intern/extern (prio dstnat+5, nach conntrack DNAT)
  accounting_out Ausgehend intern/extern (prio srcnat+5)
  dns_output    Unbound-Traffic markieren (fwmark 0x53)
  forward       gateway_only-Subnetz-Isolation
  prerouting_dns Per-Client DNS-DNAT
```

### Hintergrund-Tasks

| Task | Intervall | Funktion |
|------|-----------|----------|
| Monitor | 30s | Uplink-Ping, HA-Failover, Gruppen-Failover, Autofallback |
| Discovery | 300s | ARP/NDP-Scan, Client-Upsert |
| Traffic | 60s | nftables-Zähler → SQLite/InfluxDB |
| Interfaces | 30s | `/proc/net/dev` → InfluxDB |

### Laufzeitpfade

```
/home/stellwerk/
├── bin/stellwerk           # Binary
├── config.toml             # Konfiguration
├── stellwerk.db            # SQLite-Datenbank
├── failsafe.sh             # auto-generiert
├── apply-routing.sh        # auto-generiert
├── nftables-stellwerk.nft  # auto-generiert
├── wg-helper.sh            # auto-generiert
└── wg/                     # WireGuard-Configs (Mullvad Staging)

/etc/wireguard/             # Aktive WireGuard-Configs (mu<cc>.conf)
/etc/systemd/system/stellwerk.service
/etc/iproute2/rt_tables     # nointernet (212) eingetragen
```

Der Service läuft als User `stellwerk` mit `AmbientCapabilities=CAP_NET_ADMIN`.
**Keine File-Capabilities auf dem Binary setzen** – sie deaktivieren die Vererbung für Child-Prozesse (`ip`, `nft`, `wg`).

## Dokumentation

| Datei | Inhalt |
|-------|--------|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Technische Architektur, alle Module, DB-Schema |
| [`README_API.md`](README_API.md) | REST-API Referenz (Endpunkte, Request/Response) |
| [`config/default.toml`](config/default.toml) | Konfigurationsvorlage mit allen Optionen |

## Stack

- **Rust** – tokio, axum, sqlx, tokio-rustls, hyper 1.x
- **SQLite** – WAL-Modus, inline Migrations
- **nftables / iproute2** – Kernel-seitige Routing- und NAT-Steuerung
- **WireGuard / wg-quick** – Mullvad VPN-Tunnels
- **InfluxDB v2** – optionales Metrics-Backend
