# Stellwerk API

REST-API des Stellwerk Network Gateway Managers. Alle Responses sind JSON.

## Base URL

```
https://<host>:8443
```

Optional gibt es einen Plain-HTTP-Port für Kiosk-Clients (z.B. `http://<host>:8080`).

---

## Authentifizierung

Drei Rollen:

| Rolle | Zugriff |
|-------|---------|
| **admin** | Lesen + Schreiben |
| **viewer** | Nur Lesen |
| **kiosk** | Nur Lesen, passwortlos via Token-URL |

### Login

```http
POST /api/login
Content-Type: application/json

{ "username": "admin", "password": "meinpasswort" }
```

**Response:**
```json
{ "ok": true, "role": "admin" }
```

Der Server setzt ein `session=<token>` Cookie (HttpOnly, 24h gültig).

**Alternativ:** Token im Header senden:
```
Authorization: Bearer <token>
```

### Logout

```http
POST /api/logout
```

### Kiosk-Login (passwortlos)

```http
GET /kiosk/<kiosk_token>
```
Setzt ein Viewer-Session-Cookie mit 10 Jahren Gültigkeit und leitet auf `/` weiter.

### Aktuellen Status prüfen

```http
GET /api/me
```
```json
{ "role": "admin" }
```

---

## Clients

Ein Client ist ein LAN-Gerät, das per ARP/NDP-Discovery gefunden wurde.

### Client-Objekt

```json
{
  "ip": "172.16.1.42",
  "mac": "aa:bb:cc:dd:ee:ff",
  "hostname": "google-tv",
  "label": "Wohnzimmer TV",
  "group_name": "streaming",
  "gateway": "mullvad-de",
  "first_seen": 1711700000,
  "last_seen": 1711710000,
  "active": 1,
  "ipv6": "fe80::1",
  "dns_ip": null,
  "autofallback": 0,
  "original_gateway": null,
  "fallback_gateway": null
}
```

- `active`: 1 = im LAN gesehen, 0 = inaktiv
- `autofallback`: 1 = automatischer Failover wenn Gateway-Interface ausfällt
- `original_gateway`: gesetzt während eines aktiven Failovers (welcher Gateway vorher aktiv war)
- `fallback_gateway`: alternativer Gateway bei Failover

### Alle Clients auflisten

```http
GET /api/clients
GET /api/clients?group=streaming
GET /api/clients?gateway=mullvad-de
```

Gibt ein Array von Client-Objekten zurück.

### Einzelnen Client abrufen

```http
GET /api/clients/172.16.1.42
```

### Gateway eines Clients setzen

Wichtigste Funktion für einen TV-Client: Egress-Gateway wechseln.

```http
PUT /api/clients/172.16.1.42/gateway
Content-Type: application/json

{ "gateway": "mullvad-de" }
```

**Response:**
```json
{ "ok": true, "ip": "172.16.1.42", "gateway": "mullvad-de" }
```

Das Routing wird sofort neu angewendet.

### Label setzen

```http
PUT /api/clients/172.16.1.42/label
Content-Type: application/json

{ "label": "Wohnzimmer TV" }
```

### Gruppe zuweisen

```http
PUT /api/clients/172.16.1.42/group
Content-Type: application/json

{ "group_name": "streaming" }
```

### DNS-Server setzen

```http
PUT /api/clients/172.16.1.42/dns
Content-Type: application/json

{ "dns_ip": "1.1.1.1" }
```
Leerer String `""` entfernt den Override.

### Autofallback konfigurieren

```http
PUT /api/clients/172.16.1.42/autofallback
Content-Type: application/json

{ "fallback_gateway": "ppp0" }
```
`fallback_gateway: null` oder `""` deaktiviert den Autofallback.

---

## Gateways

### Alle Gateways auflisten

```http
GET /api/gateways
```

**Gateway-Objekt:**
```json
{
  "name": "mullvad-de",
  "table_name": "mude",
  "interface": "mude",
  "src_ip": null,
  "description": "Mullvad DE (device1)",
  "mark": 220,
  "dns_ip": "10.64.0.1",
  "device_name": "device1"
}
```

- `src_ip`: feste SNAT-IP (optional); ohne src_ip wird masquerade verwendet
- `dns_ip`: DNS-Override für alle Clients auf diesem Gateway
- Gateway-Namen für Mullvad: `<device_name>-<cc>` (z.B. `device1-de`)

### DNS-Server eines Gateways setzen

```http
PUT /api/gateways/mullvad-de/dns
Content-Type: application/json

{ "dns_ip": "10.64.0.1" }
```

---

## Gruppen

Clients können in Gruppen zusammengefasst werden. Alle Clients einer Gruppe können mit einem API-Call auf denselben Gateway umgeschaltet werden.

### Alle Gruppen auflisten

```http
GET /api/groups
```

**Gruppen-Objekt:**
```json
{
  "name": "streaming",
  "gateway": "mullvad-de",
  "fallback_gateway": "ppp0",
  "description": "Streaming-Geräte",
  "fallback_active": 0
}
```

- `fallback_active`: 1 = Gruppe ist gerade im Failover-Modus

### Gruppe erstellen / aktualisieren

```http
PUT /api/groups/streaming
Content-Type: application/json

{
  "gateway": "mullvad-de",
  "fallback_gateway": "ppp0",
  "description": "Streaming-Geräte"
}
```

### Gateway auf alle Clients einer Gruppe anwenden

```http
POST /api/groups/streaming/apply
```

Setzt den `gateway` der Gruppe auf alle Clients, die dieser Gruppe angehören.

**Response:**
```json
{ "ok": true, "updated": 3 }
```

### Gruppe löschen

```http
DELETE /api/groups/streaming
```

---

## Traffic

### Traffic der letzten 24h abrufen

```http
GET /api/traffic
```

Gibt ein Array von Traffic-Einträgen zurück (60s-Buckets):

```json
[
  {
    "ip": "172.16.1.42",
    "gateway": "mullvad-de",
    "bytes_in": 1350000,
    "bytes_out": 182700,
    "bytes_in_intern": 0,
    "bytes_out_intern": 500
  }
]
```

- `bytes_in` / `bytes_out`: Gesamt-Traffic (intern + extern)
- `bytes_in_intern` / `bytes_out_intern`: nur LAN-interner Traffic
- Extern = Gesamt minus Intern

---

## Status

```http
GET /api/status
```

```json
{
  "version": "0.1.21",
  "ppp0_up": true,
  "gre_up": true,
  "last_check": 1711710000,
  "default_gw": "gre_175",
  "scan_subnets": ["172.16.0.0/20"],
  "dns_servers": [
    ["cloudflare", "1.1.1.1"],
    ["local", "172.16.3.254"]
  ],
  "mullvad_configured": true,
  "recent_events": [
    { "id": 1, "ts": 1711700000, "event": "ppp0_down", "detail": null }
  ]
}
```

---

## Mullvad VPN

### Verfügbare Länder abrufen

```http
GET /api/mullvad/countries
```

```json
[
  { "code": "de", "name": "Germany" },
  { "code": "us", "name": "USA" }
]
```

### Aktive Mullvad-Verbindungen

```http
GET /api/mullvad/connections
```

```json
[
  {
    "country_code": "de",
    "name": "device1-de",
    "interface": "mude",
    "description": "Mullvad DE (device1)",
    "device_name": "device1"
  }
]
```

### Mit einem Land verbinden

Erstellt WireGuard-Interface + Gateway-Eintrag, wendet Routing sofort an.

```http
POST /api/mullvad/connect
Content-Type: application/json

{ "device_name": "device1", "country_code": "de" }
```

**Response:**
```json
{
  "ok": true,
  "name": "device1-de",
  "interface": "mude",
  "device_name": "device1",
  "server": "de-ber-wg-001",
  "endpoint": "1.2.3.4"
}
```

### Verbindung trennen

```http
DELETE /api/mullvad/de
```

Clients auf diesem Gateway werden automatisch auf den Default-Gateway verschoben.

### Mullvad-Geräte verwalten

```http
GET /api/mullvad/devices          # Liste aller Geräte
POST /api/mullvad/devices         # Neues Keypair erstellen + bei Mullvad registrieren
DELETE /api/mullvad/devices/:name # Gerät löschen + Key deregistrieren
```

**POST Body:**
```json
{ "name": "device1" }
```

---

## Netzwerk-Scan

Discovery + Routing-Neuanwendung anstoßen:

```http
POST /api/scan
```

---

## Monitor-Events

```http
GET /api/events
```

Letzte 50 Failover-Events:
```json
[
  { "id": 1, "ts": 1711700000, "event": "ppp0_down", "detail": null },
  { "id": 2, "ts": 1711701000, "event": "ppp0_up", "detail": null }
]
```

---

## Interfaces

```http
GET /api/ifaces
```

```json
[
  {
    "name": "eth0",
    "role": "intern",
    "enabled": 1,
    "gateways": []
  },
  {
    "name": "ppp0",
    "role": "extern",
    "enabled": 1,
    "gateways": ["ppp0"]
  }
]
```

```http
PUT /api/ifaces/eth0
Content-Type: application/json

{ "role": "intern", "enabled": true }
```

---

## Fehlerformat

Alle Fehler:
```json
{ "error": "Fehlermeldung" }
```

HTTP-Status-Codes: `200 OK`, `400 Bad Request`, `401 Unauthorized`, `403 Forbidden`, `404 Not Found`, `409 Conflict`, `500 Internal Server Error`.

---

## Typischer Flow für einen Google-TV-Client

```
1. POST /api/login                          → Session-Cookie holen
2. GET  /api/clients?gateway=default        → eigene IP finden
3. GET  /api/gateways                       → verfügbare Gateways anzeigen
4. GET  /api/mullvad/connections            → aktive VPN-Verbindungen
5. PUT  /api/clients/172.16.1.42/gateway    → Gateway wechseln
6. GET  /api/traffic                        → Traffic anzeigen
```
