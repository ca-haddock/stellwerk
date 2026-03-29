# Stellwerk – Technical Architecture v2.0

Stellwerk is a Rust daemon running on a Linux router that controls the egress gateway on a per-LAN-client basis. Routing is implemented via iproute2 policy routing (source-based), NAT and traffic accounting via nftables.

---

## System Overview

```
LAN Clients (172.16.0.0/12)
        │
        ▼
  [wall – Stellwerk]
  ┌──────────────────────────────────────────────────┐
  │  nftables: NAT + Traffic Accounting (intern/ext) │
  │  ip rule:  per client/subnet → gateway table     │
  │                                                  │
  │  ┌──────┐ ┌──────┐ ┌─────┐ ┌──────┐ ┌────────┐  │
  │  │gre175│ │vpnde │ │mude │ │ppp0  │ │noinet  │  │
  └──┴──────┴─┴──────┴─┴─────┴─┴──────┴─┴────────┴──┘
        │         │       │       │
      GRE       WireGuard Mullvad DSL      ...
        └─────────┴───────┴───────┘
                   Internet
```

**Startup sequence:**
1. Parse CLI args (`--config`, `--dry-run`) → load TOML config → open/migrate SQLite
2. Seed DNS: write `[dns].gateway` → `system_settings`, write `[dns.gateway_dns]` → `gateways.dns_ip`
3. Call `apply_routing()`: `nftables::apply_all` → `routing::apply_all` → `scripts::write_all`
4. Bring up Mullvad WireGuard interfaces in parallel (`startup_bring_up_mullvad`)
5. Start HTTP(S) server (axum + tokio-rustls; not axum-server – incompatible with hyper 1.x)
6. Spawn background tasks

---

## Modules

### `main.rs`
Entry point. Coordinates all components.

**Responsibilities:**
- Parse CLI arguments (`--config`, `--dry-run`)
- Load config, initialize database, seed DNS from config
- Call `apply_routing()` on startup and on gateway changes via API (`Notify` channel)
- Bring up Mullvad interfaces on startup (`startup_bring_up_mullvad`)
- Spawn all async tasks (`tokio::spawn`)
- Start TLS server (`serve_tls`) or plain HTTP (`axum::serve`)
- Optional second plain-HTTP listener for kiosk mode (`[api].listen_http`)

**Background tasks:**

| Task | Function | Interval |
|------|----------|----------|
| Monitor | `monitor::run_monitor_loop` | 30s |
| Discovery | `run_discovery_loop` | 300s (configurable) |
| Traffic | `traffic::run_traffic_loop` | 60s |
| Interfaces | `interfaces::run_interface_loop` | 30s |
| Reapply | Waits on `Arc<Notify>` from API | on demand |

**`apply_routing()`** calls in order:
1. `nftables::apply_all` – NAT + traffic accounting
2. `routing::apply_all` – ip rules + LAN routes
3. `scripts::write_all` – shell scripts for manual recovery
4. `scripts::configure_unbound` – Unbound upstream routing config

**TLS:** tokio-rustls + hyper (not axum's built-in TLS – axum-server is incompatible with hyper 1.x).

---

### `config.rs`
TOML configuration. All structs implement `serde::Deserialize` with `Default` implementations.

**Sections:**

| Section | Struct | Key fields |
|---------|--------|------------|
| `[db]` | `DbConfig` | `path` – SQLite file path |
| `[api]` | `ApiConfig` | `listen`, `listen_http` (optional, kiosk) |
| `[tls]` | `TlsConfig` | `enabled`, `cert`, `key` |
| `[auth]` | `AuthConfig` | `enabled`, `username`, `password_hash`, `viewer_username`, `viewer_password_hash`, `kiosk_token` |
| `[monitoring]` | `MonitoringConfig` | `check_interval_secs`, ping hosts, GRE failover settings |
| `[homeassistant]` | `HomeAssistantConfig` | `enabled`, `url`, `token`, `starlink_entity` |
| `[influxdb]` | `InfluxDbConfig` | `enabled`, `url`, `token`, `bucket`, `org` |
| `[networks]` | `NetworksConfig` | `scan_subnets`, `scan_interval_secs` |
| `[defaults]` | `DefaultsConfig` | `gateway` – default gateway for new clients |
| `[dns]` | `DnsConfig` | `gateway`, `unbound_user`, `gateway_dns` (map), `servers` (map) |
| `[mullvad]` | `MullvadConfig` | `account` – 16-digit account number |

---

### `db.rs`
SQLite database via `sqlx`. Contains all database operations and inline migration logic.

**Migrations:** Inline in `run_migrations()`. Initial schema via `CREATE TABLE IF NOT EXISTS`. New columns added as idempotent `ALTER TABLE ... ADD COLUMN` with `.ok()` (errors silently ignored if column already exists).

**Schema:**

```sql
clients (
    ip TEXT PRIMARY KEY,
    mac TEXT, hostname TEXT, label TEXT, group_name TEXT,
    gateway TEXT NOT NULL DEFAULT 'gre_175',
    first_seen INTEGER, last_seen INTEGER,
    active INTEGER DEFAULT 1,
    ipv6 TEXT,                    -- IPv6 from NDP table
    dns_ip TEXT,                  -- per-client DNS override
    autofallback INTEGER DEFAULT 0,
    original_gateway TEXT,        -- set during failover, NULL when normal
    fallback_gateway TEXT         -- used during group failover
)

gateways (
    name TEXT PRIMARY KEY,
    table_name TEXT,              -- iproute2 routing table name
    interface TEXT,
    src_ip TEXT,                  -- for SNAT; NULL = masquerade
    description TEXT,
    mark INTEGER UNIQUE,          -- fwmark
    dns_ip TEXT,                  -- per-gateway DNS override
    device_name TEXT              -- Mullvad device name
)

groups (
    name TEXT PRIMARY KEY,
    gateway TEXT NOT NULL,
    fallback_gateway TEXT,        -- HA failover target
    description TEXT,
    fallback_active INTEGER DEFAULT 0  -- 1 while in failover
)

traffic (
    id INTEGER PRIMARY KEY,
    ip TEXT, ts INTEGER,
    bytes_in INTEGER, bytes_out INTEGER,
    bytes_in_intern INTEGER,      -- LAN-internal portion
    bytes_out_intern INTEGER,
    gateway TEXT
)

networks (
    subnet TEXT PRIMARY KEY,
    default_gateway TEXT NOT NULL,
    internal_only INTEGER DEFAULT 0,  -- block internet
    gateway_only INTEGER DEFAULT 0,   -- block inter-VLAN
    dns_ip TEXT
)

interface_meta (
    name TEXT PRIMARY KEY,
    role TEXT DEFAULT 'extern',   -- 'extern' | 'intern'
    enabled INTEGER DEFAULT 1
)

mullvad_devices (
    name TEXT PRIMARY KEY,
    private_key TEXT, public_key TEXT,
    address TEXT,
    created_at INTEGER
)

monitor_events (id, ts, event, detail)
system_settings (key TEXT PRIMARY KEY, value TEXT)
```

**Key functions:**

| Function | Description |
|----------|-------------|
| `init_pool` | Open pool, run migrations, call `seed_gateways` |
| `upsert_client` | Insert client or update `last_seen`/`active` |
| `list_clients_filtered` | Filter by `group_name` and/or `gateway` |
| `set_client_gateway` | Change gateway, returns bool (found) |
| `set_client_autofallback` | Set `autofallback` + `fallback_gateway` |
| `seed_gateways` | `INSERT OR IGNORE` from hardcoded list on every startup |
| `get_traffic_24h` | Aggregated traffic per client (last 24h) |
| `upsert_group` | Create/update group |
| `apply_group_gateway` | Set all clients in group to group's gateway |
| `insert_mullvad_device` | Store WireGuard keypair + address |

---

### `routing.rs`
iproute2 policy routing. Manages `ip rule` and `ip route` entries in gateway tables.

**`apply_all(clients, gateways, networks, default_gw, dns)`:**

1. **Flush:** Delete all ip rules in priority range 999–1999
2. **prio 50** – DNS leak protection: `fwmark 0x53 → unbound_gateway table` (if configured)
3. **prio 999** – Router self-protection: `from <local_ip>/32 → main` for every router IP
4. **prio 1000** – Per-client rules: `from <client_ip> → <gateway_table>` (only for clients NOT on default gateway)
5. **prio 1500** – Per-subnet rules: from `networks` table
6. **prio 1999** – Fallback: `from 172.16.0.0/12 → default_gw table`
7. Copy RFC 1918 routes from `main` into every gateway table (LAN reachability)
8. `blackhole default table nointernet` (clients that must have no internet)

---

### `nftables.rs`
nftables rules for NAT and traffic accounting. Table name: `stellwerk` (inet family).
Table is **atomically replaced** on every apply (delete old → load new via `nft -f`).

**`build_ruleset(clients, gateways, networks, default_gw, dns)` generates:**

```
table inet stellwerk {
  counter c_172_16_x_x_in_intern {}   # 4 counters per active client
  counter c_172_16_x_x_in_extern {}
  counter c_172_16_x_x_out_intern {}
  counter c_172_16_x_x_out_extern {}

  chain postrouting {                  # NAT (priority srcnat = 100)
    type nat hook postrouting priority srcnat;
    meta mark 0x53 snat to <dns_gw.src_ip>;     # Unbound SNAT (DNS-leak protection)
    ip saddr <client> snat to <gw.src_ip>;      # per-client SNAT (if src_ip set)
    oifname "<iface>" masquerade;               # otherwise masquerade
  }

  chain accounting_out {               # outbound accounting (after SNAT)
    type filter hook postrouting priority srcnat + 5;
    ip saddr <client> ip daddr <RFC1918>     counter name c_..._out_intern;
    ip saddr <client> ip daddr != <RFC1918>  counter name c_..._out_extern;
  }

  chain accounting_in {                # inbound accounting (after DNAT restore)
    type filter hook prerouting priority dstnat + 5;  # -95, after conntrack DNAT at -100
    ip daddr <client> ip saddr <RFC1918>     counter name c_..._in_intern;
    ip daddr <client> ip saddr != <RFC1918>  counter name c_..._in_extern;
  }

  chain dns_output {                   # DNS-leak protection
    type route hook output priority mangle;
    meta skuid "unbound" meta nfproto ipv6 drop;   # force IPv4-only
    meta skuid "unbound" mark set 0x53;
  }

  chain forward {                      # gateway_only subnet isolation
    type filter hook forward priority filter;
    ip saddr <gw_only_subnet> ip daddr != <same_subnet> ip daddr <RFC1918> drop;
  }

  chain prerouting_dns {               # per-client DNS DNAT
    type nat hook prerouting priority dstnat;
    ip saddr <client_with_dns> udp dport 53 dnat to <dns_ip>;
    ip saddr <client_with_dns> tcp dport 53 dnat to <dns_ip>;
  }
}
```

**Note on `accounting_in` priority:** Uses `dstnat + 5` (-95) to run *after* conntrack DNAT restore at priority -100. This ensures `ip daddr <client_ip>` matches return internet traffic (whose destination was the router's external IP before DNAT).

**`ip_to_counter_name`:** `172.16.1.5` → `c_172_16_1_5`

---

### `mullvad.rs`
Mullvad VPN management. Interfaces named `mu<cc>` (e.g. `mude`, `muus`).

**Key functions:**

| Function | Description |
|----------|-------------|
| `generate_keypair` | Generate WireGuard private+public key via `wg genkey`/`wg pubkey` |
| `register_key` | POST to Mullvad API, returns assigned WireGuard address |
| `deregister_key` | DELETE from Mullvad API (best-effort) |
| `fetch_countries` | GET available Mullvad countries |
| `fetch_relays_for_country` | GET active relays, pick best |
| `generate_wg_config` | Build `wg-quick` config string |
| `write_wg_config` | Write to `/etc/wireguard/mu<cc>.conf` + `/home/stellwerk/wg/` |
| `bring_up` / `bring_down` | `sudo systemctl start/stop wg-quick@mu<cc>` |
| `add_default_route` | `ip route add default dev mu<cc> table mu<cc>` |
| `is_mullvad_interface` | Name starts with `mu` and is WireGuard |
| `is_wireguard_interface` | `ip link show type wireguard` check |
| `next_free_table_number` | Scan `/etc/iproute2/rt_tables`, find free slot ≥ 220 |
| `next_free_mark` | Query DB, find unused mark ≥ 220 |
| `add_rt_table_entry` / `remove_rt_table_entry` | Edit `/etc/iproute2/rt_tables` |
| `add_default_route_for` / `add_rt_table_entry_for` | Generic (non-Mullvad) WireGuard helpers |

**Requires sudoers:** `stellwerk ALL=(root) NOPASSWD: /usr/bin/systemctl start wg-quick@*, /usr/bin/systemctl stop wg-quick@*`

---

### `discovery.rs`
Network discovery: finds LAN clients via ARP and NDP.

**`discover_all(subnets)`:**
1. Per subnet: `fping -a -q -g <subnet>` (populates ARP table)
2. `read_arp_table` → `ip neigh show`, IPv4 only, only entries with `lladdr`
3. Hostname resolution via `getent hosts <ip>` (best-effort)
4. Deduplication, sorted by IP

**`read_ndp_table`:** Parses `ip -6 neigh show`, filters `fe80::` link-local. Returns `HashMap<MAC, IPv6>` → `update_ipv6_by_mac`.

---

### `traffic.rs`
Traffic accounting: reads nftables counters and stores deltas in SQLite + InfluxDB.

**Loop (every 60s):**
1. `read_counters()` – `nft -j list table inet stellwerk` (JSON)
2. Per client: read 4 counters (`_in_intern`, `_in_extern`, `_out_intern`, `_out_extern`)
3. Calculate deltas. Reset detection: if `current < prev` → counter was reset by `apply_routing()`, use `current` as delta directly
4. First measurement: set baseline only, no write (avoids inflated initial values)
5. Store deltas in SQLite (`insert_traffic`)
6. InfluxDB line protocol: `stellwerk_traffic,client=<ip>,gateway=<gw> bytes_in=...,bytes_out=...,bytes_in_intern=...,bytes_out_intern=... <ts_ns>`

**Cleanup:** Every ~168 ticks (~1 week), records older than 30 days are deleted.

---

### `interfaces.rs`
Interface statistics: reads `/proc/net/dev` and pushes to InfluxDB (only active when InfluxDB is configured).

**Loop (every 30s):** Parse `/proc/net/dev`, calculate deltas (rx/tx bytes, packets, errors, drops), push InfluxDB line protocol `stellwerk_interfaces,iface=<name> ...`.

---

### `monitor.rs`
Uplink monitoring and failover. Runs a single loop every 30s with four independent mechanisms.

**`run_monitor_loop`:**

1. **ppp0 failover** – `ping -c 1 -W 3 -I ppp0 <host>`. On failure: call HomeAssistant API to enable Starlink. Requires `[homeassistant] enabled = true`.

2. **GRE failover** (`gre_failover_enabled = true`) – On GRE interface failure: rewrite default route in all GRE routing tables to use ppp0 nexthop. Restores on recovery.

3. **Per-client autofallback** – Clients with `autofallback = 1` are monitored via their gateway's interface. On failure: save `original_gateway`, set `gateway = ppp0`. Restored on interface recovery. Fires `scan_tx.notify_one()`.

4. **Group failover** (`check_group_failover`) – Groups with a `fallback_gateway` are monitored per interface. Interface state cached per loop iteration (avoid duplicate pings). On failure: set `fallback_active = 1` in DB, fire `scan_tx`. Restored on recovery. State key in `iface_was_up`: `grp:<group_name>`.

Write current status to `Arc<RwLock<InterfaceStatus>>` (read by `/api/status`).

---

### `homeassistant.rs`
REST client for the HomeAssistant API.

- `HomeAssistantClient::new` – built once at startup, stored in `AppState`
- `turn_on_starlink` / `turn_off_starlink` – `POST /api/services/switch/turn_on|off`
- `get_starlink_state` – `GET /api/states/<entity_id>`

Only instantiated when `ha.enabled = true` and token is set.

---

### `auth.rs`
Session-based authentication.

- **Sessions:** `Arc<RwLock<HashSet<String>>>` – tokens in memory (not persisted across restart)
- **Two session sets:** `sessions` (admin) and `viewer_sessions` (read-only)
- **Token:** 32-byte random hex via `rand`
- **Password:** SHA-256 hash (no salt) – compared in `routes.rs`
- **Cookie:** `session=<token>; HttpOnly; Secure; SameSite=Strict; Max-Age=86400`
- **Token extraction:** `extract_session_token` reads from `Cookie: session=<token>` or `Authorization: Bearer <token>`
- **Kiosk:** fixed token from config, creates viewer session with 10-year cookie

---

### `scripts.rs`
Generates shell scripts to `/home/stellwerk/` for manual recovery. Regenerated on every `apply_routing()` call.

| Script | Content |
|--------|---------|
| `failsafe.sh` | Minimal routing: SSH always reachable, apply GRE default route |
| `apply-routing.sh` | All `ip rule add from <ip>` commands from current DB state |
| `nftables-stellwerk.nft` | Current nftables ruleset as `.nft` file |
| `wg-helper.sh` | Privileged WireGuard helper: `wg-quick up/down/update` |

**`configure_unbound`:** Writes Unbound config snippet for upstream routing. Creates `outgoing-interface` directive pointing to the configured DNS gateway.

---

### `api/mod.rs`
axum router setup and auth middleware.

**`AppState`** (cloned per request):

| Field | Type | Purpose |
|-------|------|---------|
| `pool` | `SqlitePool` | DB connection pool |
| `status` | `Arc<RwLock<InterfaceStatus>>` | Uplink status from monitor |
| `default_gw` | `String` | Default gateway name |
| `scan_subnets` | `Vec<String>` | Subnets for discovery |
| `scan_tx` | `Arc<Notify>` | Trigger routing reapply |
| `sessions` | `Sessions` | Admin session tokens |
| `viewer_sessions` | `Sessions` | Viewer session tokens |
| `auth_enabled` | `bool` | Whether auth is required |
| `username` / `password_hash` | `String` | Admin credentials |
| `viewer_username` / `viewer_password_hash` | `String` | Viewer credentials |
| `kiosk_token` | `String` | Fixed kiosk token |
| `dns_servers` | `Vec<(String, String)>` | Named DNS servers for UI |
| `mullvad_config` | `Option<MullvadConfig>` | Mullvad account config |
| `ha_client` | `Option<HomeAssistantClient>` | HA REST client |

**Router split in `build_router()`:**
- Public: `GET /`, `GET /kiosk/:token`, `POST /api/login`
- `read_only` (viewer + admin via `require_auth`): all GET routes + logout
- `write_only` (admin only via `require_write`): all PUT/POST/DELETE routes

---

### `api/routes.rs`
All HTTP handlers.

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/` | — | Web UI (index.html via `include_str!`) |
| GET | `/kiosk/:token` | — | Kiosk login → viewer session cookie |
| POST | `/api/login` | — | Create session, set cookie |
| POST | `/api/logout` | viewer | Delete session |
| GET | `/api/me` | viewer | Current role |
| GET | `/api/status` | viewer | Version, uplink state, recent events |
| GET | `/api/clients` | viewer | All clients; `?group=` / `?gateway=` filter |
| GET | `/api/clients/:ip` | viewer | Single client |
| PUT | `/api/clients/:ip/gateway` | admin | Assign gateway → triggers reapply |
| PUT | `/api/clients/:ip/label` | admin | Set display label |
| PUT | `/api/clients/:ip/group` | admin | Assign to group |
| PUT | `/api/clients/:ip/dns` | admin | Per-client DNS override |
| PUT | `/api/clients/:ip/autofallback` | admin | Enable/disable autofallback |
| GET | `/api/gateways` | viewer | All gateways |
| PUT | `/api/gateways/:name/dns` | admin | Per-gateway DNS override |
| GET | `/api/groups` | viewer | All groups |
| PUT | `/api/groups/:name` | admin | Create/update group (also applies gateway immediately) |
| DELETE | `/api/groups/:name` | admin | Delete group |
| POST | `/api/groups/:name/apply` | admin | Apply group's stored gateway to all its clients |
| GET | `/api/traffic` | viewer | Aggregated traffic last 24h |
| GET | `/api/events` | viewer | Last 50 monitor events |
| GET | `/api/ifaces` | viewer | Interfaces with role/enabled/gateway metadata |
| PUT | `/api/ifaces/:name` | admin | Set interface role + enabled |
| GET | `/api/networks` | viewer | All network entries |
| PUT | `/api/networks/:subnet` | admin | Upsert network config |
| GET | `/api/settings` | viewer | System settings (unbound-gateway) |
| PUT | `/api/settings/:key` | admin | Set system setting |
| POST | `/api/scan` | admin | Trigger discovery + reapply |
| POST | `/api/wg/sync` | admin | Sync active WireGuard ifaces to gateway DB |
| GET | `/api/mullvad/countries` | viewer | Available Mullvad countries |
| GET | `/api/mullvad/connections` | viewer | Active Mullvad gateways |
| GET | `/api/mullvad/devices` | viewer | Registered Mullvad devices |
| POST | `/api/mullvad/devices` | admin | Create keypair + register with Mullvad |
| DELETE | `/api/mullvad/devices/:name` | admin | Deregister key + delete device |
| POST | `/api/mullvad/connect` | admin | Connect to country (write config, bring up iface) |
| DELETE | `/api/mullvad/:cc` | admin | Disconnect + remove gateway |
| GET | `/api/stargate/status` | viewer | Starlink state via HomeAssistant |
| POST | `/api/stargate/on` | admin | Turn on Starlink |
| POST | `/api/stargate/off` | admin | Turn off Starlink |

---

## Deployment

```
/home/stellwerk/
├── bin/stellwerk           # binary
├── config.toml             # configuration (see config/default.toml)
├── stellwerk.db            # SQLite database
├── failsafe.sh             # auto-generated recovery script
├── apply-routing.sh        # auto-generated routing recovery
├── nftables-stellwerk.nft  # auto-generated nftables ruleset
├── wg-helper.sh            # auto-generated WireGuard helper
└── wg/                     # WireGuard staging configs (Mullvad)

/etc/wireguard/             # active WireGuard configs (mu<cc>.conf)
/etc/systemd/system/stellwerk.service
/etc/iproute2/rt_tables     # nointernet (212) must be registered
```

**Service:** Runs as user `stellwerk` with `AmbientCapabilities=CAP_NET_ADMIN`. Do not set file capabilities on the binary – they disable ambient caps inheritance for child processes (`ip`, `nft`, `wg`).

---

## InfluxDB Metrics

| Measurement | Tags | Fields | Interval |
|-------------|------|--------|----------|
| `stellwerk_traffic` | `client`, `gateway` | `bytes_in`, `bytes_out`, `bytes_in_intern`, `bytes_out_intern` | 60s |
| `stellwerk_interfaces` | `iface` | `rx_bytes`, `tx_bytes`, `rx_packets`, `tx_packets`, `rx_errors`, `tx_errors`, `rx_drops`, `tx_drops` | 30s |
