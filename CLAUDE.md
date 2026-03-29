# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Deploy

```bash
# Update binary + restart service (increments patch version automatically)
sudo ./bin_install.sh

# First install on a new system
sudo ./install.sh

# Safe start with connectivity test + auto-rollback
sudo ./test_start.sh

# Manual build (no install, no version bump)
cargo build --release

# Lint
cargo clippy

# No unit tests exist in this codebase
```

Service management:
```bash
systemctl enable stellwerk    # enable autostart
systemctl status stellwerk    # check status
journalctl -u stellwerk -f    # follow logs
```

Development flags:
```bash
# Run without applying nftables/iproute2 changes (safe for testing)
stellwerk --dry-run

# Override config path (default: /etc/stellwerk/config.toml)
stellwerk --config /home/stellwerk/config.toml

# Adjust log verbosity
RUST_LOG=stellwerk=debug stellwerk
```

## Architecture Overview

Stellwerk is a Linux router daemon written in Rust. It controls the **egress gateway per LAN client** using:
- **iproute2 policy routing** – per-client `ip rule` entries pointing to gateway-specific routing tables
- **nftables** – NAT/SNAT per gateway, per-client traffic accounting counters
- **SQLite** – persistent client/gateway/traffic state

### Startup Sequence

1. Parse CLI args (`--config`, `--dry-run`) → load TOML config → open SQLite (WAL mode, run migrations)
2. Seed DNS: write `[dns].gateway` → `system_settings`, write `[dns.gateway_dns]` → `gateways.dns_ip` (config.toml wins on every start)
3. Call `apply_routing()`: `nftables::apply_all` → `routing::apply_all` → `scripts::write_all` (LAN clients can route before Mullvad is up)
4. Bring up Mullvad WireGuard interfaces in parallel background tasks (`startup_bring_up_mullvad`)
5. Start HTTP(S) server (axum + tokio-rustls; **not** axum-server – incompatible with hyper 1.x)
6. Spawn background tasks

### Background Tasks

| Task | Module | Interval | Purpose |
|------|--------|----------|---------|
| Monitor | `monitor.rs` | 30s | Ping ppp0/GRE uplinks, trigger HA failover |
| Discovery | `main.rs` → `discovery.rs` | 300s (configurable) | ARP/NDP scan, upsert clients |
| Traffic | `traffic.rs` | 60s | Read nftables counters → SQLite + InfluxDB |
| Interfaces | `interfaces.rs` | 30s | `/proc/net/dev` deltas → InfluxDB |
| Reapply | `main.rs` | on demand | Wakes on `Notify` from API calls that change routing |

### Routing Logic (`routing.rs`)

`apply_all()` rebuilds all rules each time (flushes prio 999–1999 first):

1. **prio 50** – DNS leak protection: fwmark 0x53 → DNS gateway table (if configured)
2. **prio 999** – Router's own IPs → main table (self-protection)
3. **prio 1000** – Per-client rules (only for clients NOT on the default gateway)
4. **prio 1500** – Per-subnet rules (from `networks` table)
5. **prio 1999** – Fallback: `172.16.0.0/12 → default_gw table`
6. Copy private RFC 1918 routes from `main` into every gateway table (LAN reachability)
7. `blackhole default table nointernet` (for clients that must have no internet)

### nftables Structure (`nftables.rs`)

Table `inet stellwerk` is **atomically replaced** on every apply:
- Per-client named counters (`c_<ip>_in`, `c_<ip>_out`)
- `postrouting` chain: SNAT to `gw.src_ip` if set, otherwise masquerade per outgoing interface
- `accounting_in/out` chains: bump per-client counters
- `dns_output` chain: mark Unbound traffic with 0x53, drop Unbound IPv6 (DNS-leak protection)
- `forward` chain: isolate `gateway_only` subnets

### Database Schema (`db.rs`)

Key tables:
- **`clients`** – ip (PK), mac, hostname, label, group_name, gateway, first_seen, last_seen, active, ipv6, dns_ip, autofallback (0/1), original_gateway (set during failover, NULL when not in fallback)
- **`gateways`** – name (PK), table_name (iproute2 table), interface, src_ip (SNAT), description, mark, dns_ip, device_name
- **`groups`** – name (PK), gateway, fallback_gateway, description, fallback_active; clients are assigned to groups for bulk gateway management with optional HA failover
- **`interface_meta`** – name (PK), role (`extern`/`intern`), enabled; display metadata for network interfaces shown in UI
- **`traffic`** – per-client traffic deltas (60s buckets)
- **`networks`** – subnets with default_gateway, internal_only, gateway_only, dns_ip
- **`mullvad_devices`** – WireGuard key pairs for Mullvad VPN devices
- **`monitor_events`** – uplink failure/recovery log (ts, event, detail)
- **`system_settings`** – key/value store

The `dns_ip` column on clients, gateways, and networks overrides the DNS server for that entity. Applied at startup from `[dns].gateway_dns` config map.

Gateways are seeded with `INSERT OR IGNORE` on startup (`seed_gateways`). Adding a new gateway means adding it there.

### Database Migrations (`db.rs`)

Migrations run inline in `run_migrations()` on every startup — no migration files. The initial schema uses `CREATE TABLE IF NOT EXISTS`. New columns are added as idempotent `ALTER TABLE ... ADD COLUMN` statements with `.ok()` (errors silently ignored if the column already exists). When adding a new column, append it to that list.

### Auth (`auth.rs`)

Session tokens stored in `Arc<RwLock<HashSet<String>>>` (in-memory, not persisted). Two sets: admin sessions and viewer sessions. `extract_session_token` reads from cookie `session=<token>` or `Authorization: Bearer` header. Kiosk token is a fixed pre-configured value checked separately.

### API (`api/mod.rs`, `api/routes.rs`)

Router split in `build_router()`: public routes (login, kiosk) → `read_only` layer (viewers + admins via `require_auth`) → `write_only` layer (admins only via `require_write`). Middleware checks the session token from either `Cookie: session=<token>` or `Authorization: Bearer <token>`.

Three auth tiers:
- **Admin** – full read/write
- **Viewer** – read-only (all GET routes)
- **Kiosk** – passwordless, fixed token in config; served on optional `listen_http` port

Passwords stored as SHA-256 hashes (no salt) in config.toml:
```bash
echo -n "mypassword" | sha256sum | awk '{print $1}'
```

Key API routes:

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/clients` | List clients; supports `?group=<name>` and `?gateway=<name>` filters |
| PUT | `/api/clients/:ip/gateway` | Change client gateway → triggers reapply |
| PUT | `/api/clients/:ip/dns` | Set per-client DNS server |
| PUT | `/api/clients/:ip/autofallback` | Enable/disable per-client gateway auto-fallback |
| GET | `/api/groups` | List all groups (viewer) |
| PUT | `/api/groups/:name` | Create/update group (with `gateway` + `fallback_gateway`) |
| DELETE | `/api/groups/:name` | Delete group |
| POST | `/api/groups/:name/apply` | Apply group's gateway to all clients in the group |
| PUT | `/api/gateways/:name/dns` | Set per-gateway DNS server |
| PUT | `/api/ifaces/:name` | Set interface metadata (role, enabled) |
| POST | `/api/scan` | Trigger discovery + reapply routing |
| POST | `/api/wg/sync` | Sync WireGuard configs from DB to disk |
| GET/POST/DELETE | `/api/stargate/*` | HomeAssistant Stargate switch control |
| POST | `/api/mullvad/devices` | Generate keypair + register with Mullvad |
| GET | `/api/mullvad/devices` | List registered Mullvad devices |
| DELETE | `/api/mullvad/devices/:name` | Deregister key + remove device |
| GET | `/api/mullvad/countries` | Fetch available Mullvad countries |
| GET | `/api/mullvad/connections` | List active Mullvad gateways |
| POST | `/api/mullvad/connect` | Connect to a Mullvad country (pick best relay) |
| DELETE | `/api/mullvad/:cc` | Disconnect Mullvad country interface |

API calls that change routing call `scan_tx.notify_one()` to wake the reapply loop.

### Auto-Generated Recovery Scripts (`scripts.rs`)

Regenerated on every `apply_routing()` call into `/home/stellwerk/`:
- `failsafe.sh` – minimal routing recovery (SSH always reachable)
- `apply-routing.sh` – recreates all `ip rule` entries from current DB
- `nftables-stellwerk.nft` – current nftables ruleset
- `wg-helper.sh` – privileged WireGuard helper (wg-quick up/down/update)

### Mullvad VPN (`mullvad.rs`)

Optional. Requires `[mullvad] account = "..."` in config. Registers WireGuard keypairs with the Mullvad API, stores devices in `mullvad_devices` table.

- Interfaces named `mu<cc>` (e.g. `mude`, `muus`)
- Routing tables allocated from 220 upward, marks from 220 upward
- Configs written to `/etc/wireguard/mu<cc>.conf` and staged in `/home/stellwerk/wg/`
- `bring_up`/`bring_down` use `sudo systemctl start/stop wg-quick@mu<cc>` — requires sudoers entry: `stellwerk ALL=(root) NOPASSWD: /usr/bin/systemctl start wg-quick@*, /usr/bin/systemctl stop wg-quick@*`
- `mullvad.rs` also provides helpers for generic interfaces: `add_rt_table_entry_for`, `add_default_route_for`, `iface_exists`, `is_wireguard_interface` (checks `ip link show type wireguard`)

### HomeAssistant Integration (`homeassistant.rs`)

`HomeAssistantClient` wraps `reqwest` calls to the HA REST API. Used in two places:
- `monitor.rs` – automatic failover (calls switch service on uplink failure)
- `api/routes.rs` – manual stargate control via `/api/stargate/on|off|status`

Requires `[homeassistant] enabled = true` in config. The client is built once at startup and stored in `AppState`.

### Monitor & Failover (`monitor.rs`)

Three independent failover mechanisms run in the same 30s loop:

1. **ppp0 failover** – on ppp0 failure, calls HomeAssistant API to enable Starlink. Requires `[homeassistant] enabled = true`.
2. **GRE failover** (`gre_failover_enabled = true`) – on GRE interface failure, rewrites the default route in all GRE routing tables to use ppp0. Restores on recovery. Configured via `gre_interface` and `gre_nexthop`.
3. **Per-client autofallback** – clients with `autofallback = 1` are monitored by their gateway's interface. On failure, `original_gateway` is saved and `gateway` is set to `ppp0`; restored when the interface recovers. Triggers `scan_tx.notify_one()`.
4. **Group failover** (`check_group_failover`) – groups with a `fallback_gateway` are monitored per interface. Interface state is cached within each loop iteration to avoid redundant pings. On failure, `fallback_active` is set (DB) and `scan_tx.notify_one()` fires; restored on recovery. State key in `iface_was_up`: `grp:<group_name>`.

### Configuration (`config.rs`, `config/default.toml`)

Notable non-obvious config sections:

```toml
[api]
listen_http = "0.0.0.0:8080"   # optional plain HTTP listener (kiosk mode)

[dns]
gateway = "vpnde"               # route Unbound upstream queries via this gateway
unbound_user = "unbound"        # Linux user Unbound runs as

[dns.gateway_dns]               # override DNS server per gateway (written to DB on startup)
vpnde = "1.1.1.1"
vpnus = "1.0.0.1"

[dns.servers]                   # named DNS servers shown in UI dropdowns
local      = "172.16.3.254"
cloudflare = "1.1.1.1"

[mullvad]
account = "1234567890123456"    # 16-digit Mullvad account number
```

## Key Runtime Paths

```
/home/stellwerk/
├── bin/stellwerk           # binary
├── config.toml             # configuration (see config/default.toml for reference)
├── stellwerk.db            # SQLite database
├── failsafe.sh             # auto-generated
├── apply-routing.sh        # auto-generated
├── nftables-stellwerk.nft  # auto-generated
└── wg/                     # WireGuard configs (Mullvad staging copies)

/etc/wireguard/             # active WireGuard configs (mu<cc>.conf)
/etc/systemd/system/stellwerk.service
/etc/iproute2/rt_tables     # nointernet (212) must be registered here
```

The service runs as user `stellwerk` with `AmbientCapabilities=CAP_NET_ADMIN`. **Do not set file capabilities on the binary** – they disable ambient caps inheritance for child processes (`ip`, `nft`, `wg`).

## Version Management

`bin_install.sh` auto-increments the patch version in `Cargo.toml` before every build. The version is embedded via `env!("CARGO_PKG_VERSION")` and exposed at `GET /api/status`.
