> **Dieses Dokument ist veraltet.**
> Die vollständige und aktuelle API-Dokumentation befindet sich in [`README_API.md`](README_API.md).

---

# Stellwerk API – curl Beispiele

Praktische curl-Beispiele für die häufigsten API-Aufrufe.

## Token holen

```bash
TOKEN=$(curl -si -X POST https://stellwerk.local/api/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "deinpasswort"}' \
  | grep -i set-cookie | sed 's/.*session=\([^;]*\).*/\1/')
```

## Gateway eines Clients wechseln

```bash
curl -s -X PUT https://stellwerk.local/api/clients/172.16.1.42/gateway \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"gateway": "mullvad-de"}'
```

## Alle Clients einer Gruppe auf neuen Gateway

```bash
# Gruppe aktualisieren
curl -s -X PUT https://stellwerk.local/api/groups/streaming \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"gateway": "mullvad-de", "fallback_gateway": "ppp0"}'

# Gateway sofort auf alle Clients der Gruppe anwenden
curl -s -X POST https://stellwerk.local/api/groups/streaming/apply \
  -H "Authorization: Bearer $TOKEN"
```

## Mullvad verbinden

```bash
curl -s -X POST https://stellwerk.local/api/mullvad/connect \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"device_name": "device1", "country_code": "de"}'
```

## Mullvad trennen

```bash
curl -s -X DELETE https://stellwerk.local/api/mullvad/de \
  -H "Authorization: Bearer $TOKEN"
```

## Status abrufen

```bash
curl -s https://stellwerk.local/api/status \
  -H "Authorization: Bearer $TOKEN"
```

## Netzwerk-Scan auslösen

```bash
curl -s -X POST https://stellwerk.local/api/scan \
  -H "Authorization: Bearer $TOKEN"
```

## Passwort-Hash generieren

```bash
echo -n "meinpasswort" | sha256sum | awk '{print $1}'
```

---

Vollständige Dokumentation aller Endpunkte, Request-Bodies und Response-Formate: [`README_API.md`](README_API.md)
