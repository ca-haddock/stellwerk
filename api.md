# Stellwerk API – Howto

## Authentifizierung

### Login (Token holen)

```bash
curl -s -X POST https://stellwerk.local/api/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "deinpasswort"}'
```

Antwort:
```json
{"ok": true, "role": "admin"}
```

Der Token wird als `Set-Cookie: session=<token>` Header zurückgegeben. Es gibt zwei Rollen:
- **admin** – Lesen + Schreiben
- **viewer** – Nur lesen

### Token extrahieren (für Bearer-Auth)

```bash
TOKEN=$(curl -si -X POST https://stellwerk.local/api/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "deinpasswort"}' \
  | grep -i set-cookie | sed 's/.*session=\([^;]*\).*/\1/')

echo $TOKEN
```

### Token verwenden

**Option A – Cookie-Datei:**
```bash
curl -s https://stellwerk.local/api/clients -c cookies.txt  # beim Login Cookie speichern
curl -s https://stellwerk.local/api/clients -b cookies.txt  # Cookie wiederverwenden
```

**Option B – Bearer Header:**
```bash
curl -s https://stellwerk.local/api/clients \
  -H "Authorization: Bearer $TOKEN"
```

### Token löschen (Logout)

```bash
curl -s -X POST https://stellwerk.local/api/logout \
  -H "Authorization: Bearer $TOKEN"
```

Antwort: `{"ok": true}` – der Token wird serverseitig ungültig gemacht und das Cookie gelöscht.

---

## Clients

### Alle Clients abrufen

```bash
curl -s https://stellwerk.local/api/clients \
  -H "Authorization: Bearer $TOKEN"
```

### Clients nach Gruppe filtern

```bash
curl -s "https://stellwerk.local/api/clients?group=familie" \
  -H "Authorization: Bearer $TOKEN"
```

### Clients nach Gateway filtern

```bash
curl -s "https://stellwerk.local/api/clients?gateway=vpnde" \
  -H "Authorization: Bearer $TOKEN"
```

### Kombination: Gruppe + Gateway

```bash
curl -s "https://stellwerk.local/api/clients?group=familie&gateway=vpnde" \
  -H "Authorization: Bearer $TOKEN"
```

### Einzelnen Client abrufen

```bash
curl -s https://stellwerk.local/api/clients/192.168.1.42 \
  -H "Authorization: Bearer $TOKEN"
```

### Client-Felder

| Feld | Typ | Beschreibung |
|------|-----|--------------|
| `ip` | string | IP-Adresse (Primary Key) |
| `mac` | string\|null | MAC-Adresse |
| `hostname` | string\|null | Hostname (aus DNS/DHCP) |
| `label` | string\|null | Manuell vergebener Name |
| `group_name` | string\|null | Gruppe |
| `gateway` | string | Aktuelles Gateway |
| `active` | int | 1 = aktiv, 0 = inaktiv |
| `first_seen` | int | Unix-Timestamp |
| `last_seen` | int | Unix-Timestamp |
| `ipv6` | string\|null | IPv6-Adresse |
| `dns_ip` | string\|null | Individueller DNS-Server |
| `autofallback` | int | 1 = automatischer Fallback aktiv |
| `original_gateway` | string\|null | Gateway vor Fallback |

---

## Gateways

### Alle Gateways abrufen

```bash
curl -s https://stellwerk.local/api/gateways \
  -H "Authorization: Bearer $TOKEN"
```

### Gateway-Felder

| Feld | Typ | Beschreibung |
|------|-----|--------------|
| `name` | string | Name (Primary Key) |
| `table_name` | string | iproute2-Tabelle |
| `interface` | string | Netzwerk-Interface |
| `src_ip` | string\|null | SNAT-Quell-IP |
| `description` | string\|null | Beschreibung |
| `mark` | int | Firewall-Mark |
| `dns_ip` | string\|null | DNS-Server für dieses Gateway |
| `device_name` | string\|null | Gerätename (Mullvad) |

---

## Client-Einstellungen ändern (Admin)

### Gateway zuweisen

```bash
curl -s -X PUT https://stellwerk.local/api/clients/192.168.1.42/gateway \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"gateway": "vpnde"}'
```

Antwort: `{"ok": true, "ip": "192.168.1.42", "gateway": "vpnde"}`

Das Routing wird sofort neu berechnet.

### Label setzen

```bash
curl -s -X PUT https://stellwerk.local/api/clients/192.168.1.42/label \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"label": "Mein Laptop"}'
```

### Gruppe setzen

```bash
curl -s -X PUT https://stellwerk.local/api/clients/192.168.1.42/group \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"group_name": "familie"}'
```

Leerer `group_name` entfernt den Client aus der Gruppe.

### DNS-Server setzen

```bash
curl -s -X PUT https://stellwerk.local/api/clients/192.168.1.42/dns \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"dns_ip": "1.1.1.1"}'
```

Leere `dns_ip` setzt auf Standard zurück.

---

## Gruppen

### Alle Gruppen abrufen

```bash
curl -s https://stellwerk.local/api/groups \
  -H "Authorization: Bearer $TOKEN"
```

### Gruppe anlegen / aktualisieren

```bash
curl -s -X PUT https://stellwerk.local/api/groups/familie \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"gateway": "vpnde", "description": "Familien-Geräte"}'
```

### Gateway für alle Clients einer Gruppe setzen

```bash
curl -s -X POST https://stellwerk.local/api/groups/familie/apply \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"gateway": "vpnde"}'
```

---

## System

### Status abrufen

```bash
curl -s https://stellwerk.local/api/status \
  -H "Authorization: Bearer $TOKEN"
```

Enthält: Version, ppp0/GRE-Zustand, Default-Gateway, Scan-Subnetze, letzte Events.

### Netzwerk-Scan auslösen

```bash
curl -s -X POST https://stellwerk.local/api/scan \
  -H "Authorization: Bearer $TOKEN"
```

### Aktuell angemeldete Rolle prüfen

```bash
curl -s https://stellwerk.local/api/me \
  -H "Authorization: Bearer $TOKEN"
```

Antwort: `{"role": "admin"}` oder `{"role": "viewer"}`

---

## Passwort-Hash generieren

Passwörter werden als SHA-256 gespeichert (kein Salt):

```bash
echo -n "meinpasswort" | sha256sum | awk '{print $1}'
```

---

## Fehler-Codes

| Code | Bedeutung |
|------|-----------|
| 200 | OK |
| 400 | Ungültige Anfrage (z.B. unbekanntes Gateway) |
| 401 | Nicht authentifiziert |
| 403 | Nur-Lese-Zugriff (write-Endpunkt mit viewer-Token) |
| 404 | Ressource nicht gefunden |
| 500 | Interner Fehler |
