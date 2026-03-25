# Stellwerk – Technical Architecture

Stellwerk is a Rust daemon running on a Linux router that controls the egress gateway on a per-LAN-client basis. Routing is implemented via iproute2 policy routing (source-based), NAT and traffic accounting via nftables.

---

## System Overview

```
LAN Clients (172.16.0.0/12)
        │
        ▼
  [wall – Stellwerk]
  ┌─────────────────────────────────────────┐
  │  nftables: NAT + Traffic Accounting     │
  │  ip rule:  per client → gateway table   │
  │                                         │
  │  ┌──────┐ ┌──────┐ ┌────────┐ ┌──────┐ │
  │  │gre175│ │vpnde │ │stargate│ │ ... │ │
  └──┴──────┴─┴──────┴─┴────────┴─┴──────┴─┘
        │         │         │
      GRE       WireGuard  Starlink  ...
        └─────────┴─────────┘
              Internet
```

**Startup data flow:**
1. Load config → open/migrate database
2. Apply routing (nftables + ip rules + copy LAN routes)
3. Start HTTP(S) API (axum + tokio-rustls)
4. Spawn background tasks: Discovery, Monitor, Traffic, Interfaces

---

## Modules

### `main.rs`
Entry point. Coordinates all components.

**Responsibilities:**
- Parse CLI arguments (`--config`, `--dry-run`)
- Load config, initialize database
- Call `apply_routing()` on startup and on gateway changes via API (Notify channel)
- Spawn all async tasks (`tokio::spawn`)
- Start TLS server (`serve_tls`) or plain HTTP (`axum::serve`)

**Task overview:**

| Task | Function | Interval |
|------|----------|----------|
| Monitor | `monitor::run_monitor_loop` | 30s |
| Discovery | `run_discovery_loop` | 300s (configurable) |
| Traffic | `traffic::run_traffic_loop` | 60s |
| Interfaces | `interfaces::run_interface_loop` | 30s |
| Reapply | Waits on `Notify` from API | on demand |

**`apply_routing()`** calls in order:
1. `nftables::apply_all` – NAT + accounting
2. `routing::apply_all` – ip rules + LAN routes
3. `scripts::write_all` – shell scripts for manual recovery

**TLS:** tokio-rustls + hyper (not axum's built-in TLS), because axum-server had no rustls compatibility.

---

### `config.rs`
TOML configuration. All structs implement `serde::Deserialize` with sensible `Default` implementations.

**Sections:**

| Section | Struct | Key fields |
|---------|--------|------------|
| `[db]` | `DbConfig` | `path` – SQLite file path |
| `[api]` | `ApiConfig` | `listen` – bind address |
| `[tls]` | `TlsConfig` | `enabled`, `cert`, `key` |
| `[auth]` | `AuthConfig` | `enabled`, `username`, `password_hash` (SHA-256) |
| `[monitoring]` | `MonitoringConfig` | `check_interval_secs`, ping hosts |
| `[homeassistant]` | `HomeAssistantConfig` | `url`, `token`, `starlink_entity` |
| `[influxdb]` | `InfluxDbConfig` | `url`, `token`, `bucket`, `org` |
| `[networks]` | `NetworksConfig` | `scan_subnets`, `scan_interval_secs` |
| `[defaults]` | `DefaultsConfig` | `gateway` – default gateway for new clients |

---

### `db.rs`
SQLite database via `sqlx`. Contains all database operations and migration logic.

**Schema:**

```sql
clients (
    ip TEXT PRIMARY KEY,
    mac TEXT, hostname TEXT, label TEXT, group_name TEXT,
    gateway TEXT NOT NULL DEFAULT 'gre_175',
    first_seen INTEGER, last_seen INTEGER,
    active INTEGER DEFAULT 1,
    ipv6 TEXT                          -- IPv6 address from NDP table
)

gateways (
    name TEXT PRIMARY KEY,
    table_name TEXT,                   -- iproute2 routing table name
    interface TEXT,
    src_ip TEXT,                       -- for SNAT, optional
    description TEXT,
    mark INTEGER UNIQUE                -- fwmark (legacy, currently unused for routing)
)

traffic (
    id INTEGER PRIMARY KEY,
    ip TEXT, ts INTEGER,
    bytes_in INTEGER, bytes_out INTEGER,
    gateway TEXT
)

monitor_events (
    id INTEGER PRIMARY KEY,
    ts INTEGER, event TEXT, detail TEXT
)
```

**Key functions:**

| Function | Description |
|----------|-------------|
| `init_pool` | Open pool, run migrations and seed |
| `upsert_client` | Insert client or update `last_seen`/`active` |
| `update_ipv6_by_mac` | Assign IPv6 address to client by MAC |
| `set_client_gateway` | Change a client's gateway |
| `seed_gateways` | Insert gateways via `INSERT OR IGNORE` |
| `get_traffic_24h` | Aggregated traffic per client (last 24h) |
| `cleanup_old_traffic` | Delete records older than N days (weekly) |

**Gateways (hardcoded in `seed_gateways`):**

| Name | Table | Interface | Mark |
|------|-------|-----------|------|
| gre_175 | gre_175 | gre_fiber | 175 |
| gre_214 | gre_214 | gre_fiber | 214 |
| gre_215 | gre_215 | gre_fiber | 215 |
| vpnde | vpnde | vpnfra | 204 |
| vpnus | vpnus | vpnusa | 205 |
| webgate | webgate | vpnagn | 207 |
| stargate | stargate | enp1s0.12 | 208 |
| buda | buda | buda | 203 |
| mobile | mobile | mobile | 209 |
| ppp0 | main | ppp0 | 100 |
| nointernet | nointernet | lo | 212 |

---

### `routing.rs`
iproute2 policy routing. Manages `ip rule` and `ip route` entries in gateway tables.

**`apply_all(clients, gateways, default_gw)`** – main function:

1. **Flush:** Delete all ip rules in priority range 999–1999 (`flush_stellwerk_rules`)
2. **Per-client rules** (prio 1000): `ip rule add from <ip> lookup <table>`
   Only for clients NOT on the default gateway.
3. **Router self-protection** (prio 999): For every local IP of the router:
   `ip rule add from <local_ip>/32 lookup main`
   → Prevents the router's own traffic from being misrouted through gateway tables.
4. **Fallback** (prio 1999): `ip rule add from 172.16.0.0/12 lookup gre_175`
   → All LAN clients without a specific rule → default gateway.
5. **Copy LAN routes:** All private routes (10.x, 172.16-31.x, 192.168.x) from the main
   table are copied into every gateway table.
   → Clients on e.g. `gre_215` can still reach other LAN subnets.
6. **nointernet:** `ip route replace blackhole default table nointernet`
   → Internet traffic is silently dropped, LAN routes remain (from step 5).

**`local_ips()`:** Reads all local IPv4 addresses from `ip addr show` (excluding loopback).

**`is_private_route(line)`:** Checks whether a route line belongs to an RFC 1918 private network.

---

### `nftables.rs`
nftables rules for NAT and traffic accounting. Table name: `stellwerk` (inet family).

**`build_ruleset(clients, gateways, default_gw)`** generates:

```
table inet stellwerk {
  counter c_172_16_x_x_in {}     # one per active client
  counter c_172_16_x_x_out {}

  chain postrouting {             # NAT
    type nat hook postrouting priority srcnat;
    ip saddr <client> snat to <gw.src_ip>;  # SNAT if src_ip is set
    oifname "<iface>" masquerade;            # otherwise masquerade
  }

  chain accounting_out {          # outbound traffic
    type filter hook postrouting priority srcnat + 5;
    ip saddr <client> counter name c_..._out;
  }

  chain accounting_in {           # inbound traffic
    type filter hook prerouting priority mangle + 5;
    ip daddr <client> counter name c_..._in;
  }
}
```

**`apply_ruleset`:** Atomically deletes the old table and loads the new one via `nft -f /tmp/stellwerk-nft.conf`.

**`read_counters`:** Reads byte counters via `nft -j list table inet stellwerk` (JSON output).

**`ip_to_counter_name`:** `172.16.1.5` → `c_172_16_1_5`

---

### `discovery.rs`
Network discovery: finds LAN clients via ARP and NDP.

**`discover_all(subnets)`:**
1. Per subnet: `ping_sweep` → `fping -a -q -g <subnet>` (populates ARP table)
2. `read_arp_table` → `ip neigh show`, IPv4 only, only entries with `lladdr`
3. Hostname resolution via `getent hosts <ip>` (best-effort)
4. Deduplication, sorted by IP

**`read_arp_table`:** Parses `ip neigh show`, filters out IPv6 and FAILED/INCOMPLETE entries.

**`read_ndp_table`:** Parses `ip -6 neigh show`, filters out `fe80::` link-local addresses.
Returns `HashMap<MAC, IPv6>` → written to DB via `update_ipv6_by_mac`.

---

### `traffic.rs`
Traffic accounting: reads nftables counters and stores deltas in SQLite + InfluxDB.

**Loop (every 60s):**
1. `read_counters()` – read nftables JSON output
2. Per client: calculate delta from previous measurement (`saturating_sub`)
3. First measurement: set baseline only, no push (avoids inflated initial values)
4. Store delta in SQLite (`insert_traffic`)
5. InfluxDB line protocol: `stellwerk_traffic,client=<ip>,gateway=<gw> bytes_in=...,bytes_out=... <ts>`

**Cleanup:** Every ~168 ticks (~1 week), traffic records older than 30 days are deleted.

---

### `interfaces.rs`
Interface statistics: reads `/proc/net/dev` and pushes to InfluxDB.

**Loop (every 30s):**
1. Parse `/proc/net/dev` – all interfaces except `lo`
2. Calculate deltas (rx/tx bytes, packets, errors, drops)
3. New interfaces are detected automatically on the next tick (no restart needed)
4. InfluxDB line protocol: `stellwerk_interfaces,iface=<name> rx_bytes=...,tx_bytes=...,rx_errors=...,tx_errors=...,rx_drops=...,tx_drops=...,rx_packets=...,tx_packets=... <ts>`

**/proc/net/dev column mapping:**

| Column | Field |
|--------|-------|
| 0 | rx_bytes |
| 1 | rx_packets |
| 2 | rx_errors |
| 3 | rx_drops |
| 8 | tx_bytes |
| 9 | tx_packets |
| 10 | tx_errors |
| 11 | tx_drops |

---

### `monitor.rs`
Monitors uplink interfaces and notifies HomeAssistant on failure.

**`run_monitor_loop` (every 30s):**
1. `ping -c 1 -W 3 -I ppp0 <ppp0_check_host>` → ppp0 status
2. `ping -c 1 -W 3 <gre_check_host>` → GRE status
3. Write status to `Arc<RwLock<InterfaceStatus>>` (read by API)
4. On ppp0 failure: call HomeAssistant → enable Starlink
5. On ppp0 recovery: log event

---

### `homeassistant.rs`
REST client for the HomeAssistant API.

**Endpoint:** `POST /api/services/switch/turn_on` with `entity_id = starlink_entity`

Only called when `ha.enabled = true` and token is set.

---

### `auth.rs`
Session-based authentication.

- **Sessions:** `Arc<RwLock<HashSet<String>>>` – tokens in memory (not persisted)
- **Token:** 32-byte random hex via `rand`
- **Password:** SHA-256 hash (no salt) – compared in `routes.rs`
- **Cookie:** `session=<token>; HttpOnly; Secure; SameSite=Strict; Max-Age=86400`
- Token extraction: from `Cookie` header or `Authorization: Bearer`

---

### `api/mod.rs`
axum router setup and auth middleware.

**`AppState`** (cloned per request):
- `pool` – DB connection pool
- `status` – uplink status (from monitor)
- `sessions` – active sessions
- `scan_tx` – Notify for manual scan/reapply trigger
- `auth_enabled`, `username`, `password_hash`

**Auth middleware `require_auth`:** Checks session token, returns 401 if not found.
Unauthenticated routes: `GET /` and `POST /api/login`.

---

### `api/routes.rs`
All HTTP handlers.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Web UI (index.html, embedded via `include_str!`) |
| POST | `/api/login` | Create session, set cookie |
| POST | `/api/logout` | Delete session |
| GET | `/api/status` | ppp0/GRE status + recent events |
| GET | `/api/clients` | All clients |
| GET | `/api/clients/:ip` | Single client |
| PUT | `/api/clients/:ip/gateway` | Assign gateway → triggers reapply |
| PUT | `/api/clients/:ip/label` | Set label |
| PUT | `/api/clients/:ip/group` | Set group |
| GET | `/api/gateways` | All gateways |
| GET | `/api/traffic` | Aggregated traffic last 24h |
| GET | `/api/events` | Last 50 monitor events |
| POST | `/api/scan` | Trigger discovery + reapply |

---

### `scripts.rs`
Generates shell scripts to `/home/stellwerk/` for manual recovery operation.

| Script | Content |
|--------|---------|
| `failsafe.sh` | Minimal routing: set default via GRE, ensure SSH access |
| `apply-routing.sh` | All `ip rule add from <ip>` commands from current DB state |
| `nftables-stellwerk.nft` | Current nftables ruleset as a `.nft` file |

These scripts are regenerated on every `apply_routing()` call and always reflect the current DB state. They are intended for use when Stellwerk is not running.

---

## Deployment

```
/home/stellwerk/
├── bin/stellwerk           # binary
├── config.toml             # configuration
├── stellwerk.db            # SQLite database
├── failsafe.sh             # recovery script (auto-generated)
├── apply-routing.sh        # routing recovery (auto-generated)
└── nftables-stellwerk.nft  # nftables ruleset (auto-generated)

/etc/systemd/system/stellwerk.service
/etc/iproute2/rt_tables     # nointernet (212) registered
```

**Service management:**
```bash
sudo ./install.sh           # first install (starts without enable)
sudo ./bin_install.sh       # update binary + restart
sudo ./test_start.sh        # safe start with connectivity test + auto-rollback
systemctl enable stellwerk  # enable autostart
systemctl disable stellwerk # disable autostart
```

**Capabilities:** Stellwerk runs as user `stellwerk` with `AmbientCapabilities=CAP_NET_ADMIN` in the systemd service. This allows child processes (`ip`, `nft`) to inherit CAP_NET_ADMIN without needing setuid or running as root.

---

## InfluxDB Metrics

**Bucket:** `wall` | **Org:** `stonetech`

| Measurement | Tag | Fields | Interval |
|-------------|-----|--------|----------|
| `stellwerk_traffic` | `client`, `gateway` | `bytes_in`, `bytes_out` | 60s |
| `stellwerk_interfaces` | `iface` | `rx_bytes`, `tx_bytes`, `rx_packets`, `tx_packets`, `rx_errors`, `tx_errors`, `rx_drops`, `tx_drops` | 30s |
