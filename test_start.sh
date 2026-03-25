#!/bin/bash
# Stellwerk sicher starten und Netzwerk-Erreichbarkeit testen.
# Bei Fehler: Service sofort stoppen (kein Autostart!).
#
# Verwendung: sudo ./test_start.sh [GATEWAY_IP]
#   Ohne Argument: Default-Gateway aus 'ip route' ermitteln

set -euo pipefail

TIMEOUT=10       # Sekunden Wartezeit nach Start bis zum Test
PING_COUNT=5     # Ping-Versuche
PING_WAIT=2      # Sekunden zwischen Pings (bei Fehler)

# ---- Root-Check ----
if [[ $EUID -ne 0 ]]; then
    echo "Fehler: root erforderlich." >&2
    exit 1
fi

# ---- Gateway ermitteln ----
if [[ -n "${1:-}" ]]; then
    GW="$1"
    echo "==> Verwende angegebenes Gateway: $GW"
else
    # Versuche via-IP aus Default-Route
    GW=$(ip route show default | awk '/default via/ {print $3; exit}')

    # Fallback: ppp0 Peer-IP (Point-to-Point ohne via)
    if [[ -z "$GW" ]]; then
        GW=$(ip addr show ppp0 2>/dev/null | awk '/inet .* peer / {split($4,a,"/"); print a[1]}')
    fi

    # Letzter Fallback: bekannter öffentlicher Host
    if [[ -z "$GW" ]]; then
        GW="8.8.8.8"
        echo "==> Kein Gateway ermittelt, nutze Fallback: $GW"
    else
        echo "==> Gateway erkannt: $GW"
    fi
fi

# ---- SSH-Client-IP ermitteln ----
SSH_IP=""

# 1. Direkt aus Umgebungsvariable (funktioniert wenn sudo env_keep gesetzt)
if [[ -n "${SSH_CLIENT:-}" ]]; then
    SSH_IP=$(echo "$SSH_CLIENT" | awk '{print $1}')
elif [[ -n "${SSH_CONNECTION:-}" ]]; then
    SSH_IP=$(echo "$SSH_CONNECTION" | awk '{print $1}')
fi

# 2. Aus dem Parent-Prozess-Environment (sudo-Fall)
if [[ -z "$SSH_IP" && -f "/proc/$PPID/environ" ]]; then
    SSH_IP=$(tr '\0' '\n' < "/proc/$PPID/environ" 2>/dev/null \
        | grep '^SSH_CLIENT=' | cut -d= -f2- | awk '{print $1}' || true)
fi

# 3. Fallback: aktive SSH-Verbindungen via ss
# Spalten: Recv-Q Send-Q Local:Port Peer:Port Process
if [[ -z "$SSH_IP" ]]; then
    SSH_IP=$(ss -tnp state established '( sport = :22 )' 2>/dev/null \
        | awk 'NR>1 {split($4,a,":"); print a[1]}' | head -1 || true)
fi

# Validierung: muss wie eine IPv4-Adresse aussehen
if [[ -n "$SSH_IP" ]] && ! echo "$SSH_IP" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "==> Warnung: SSH-Client-Erkennung lieferte ungültige IP '$SSH_IP' – überspringe"
    SSH_IP=""
fi

if [[ -n "$SSH_IP" ]]; then
    echo "==> SSH-Client erkannt: $SSH_IP"
else
    echo "==> Warnung: SSH-Client-IP nicht ermittelbar – SSH-Test wird übersprungen"
fi

# ---- Ping-Hilfsfunktion ----
ping_host() {
    local host="$1" label="$2"
    local ok=0
    for i in $(seq 1 "$PING_COUNT"); do
        if ping -c 1 -W 2 "$host" &>/dev/null; then
            ok=$((ok + 1))
        else
            echo "    Ping $i/$PING_COUNT nach $label fehlgeschlagen"
            sleep "$PING_WAIT"
        fi
    done
    echo "==> $label: $ok/$PING_COUNT Pings erfolgreich"
    [[ "$ok" -eq "$PING_COUNT" ]]
}

# ---- Rollback-Funktion ----
rollback() {
    local reason="$1"
    echo ""
    echo "!!! FEHLER: $reason !!!"
    echo "==> Stoppe stellwerk..."
    systemctl stop stellwerk
    echo "==> stellwerk gestoppt. Routing sollte wiederhergestellt sein."
    echo ""
    echo "Diagnose:"
    echo "  journalctl -u stellwerk -n 50 --no-pager"
    echo "  ip rule show"
    echo "  ip route show"
    exit 1
}

# ---- Vorher: Erreichbarkeit prüfen ----
echo "==> Prüfe Erreichbarkeit VOR Start..."
if ! ping -c 2 -W 3 "$GW" &>/dev/null; then
    echo "Warnung: Gateway $GW bereits VOR dem Start nicht erreichbar."
fi
if [[ -n "$SSH_IP" ]] && ! ping -c 2 -W 3 "$SSH_IP" &>/dev/null; then
    echo "Warnung: SSH-Client $SSH_IP bereits VOR dem Start nicht per Ping erreichbar."
    echo "         (Firewall auf Client-Seite? Test läuft trotzdem.)"
fi

# ---- Sicherstellen: Service NICHT enabled ----
if systemctl is-enabled stellwerk &>/dev/null; then
    echo "==> Deaktiviere Boot-Autostart (systemctl disable stellwerk)..."
    systemctl disable stellwerk
    echo "    Stellwerk startet nach Reboot NICHT mehr automatisch."
fi

# ---- Stellwerk starten ----
echo "==> Starte stellwerk..."
systemctl start stellwerk

echo "==> Warte ${TIMEOUT}s damit Routing-Regeln greifen..."
sleep "$TIMEOUT"

# ---- Tests nach Start ----
echo ""
echo "==> Netzwerk-Tests nach Start:"

FAILED=""

if ! ping_host "$GW" "Gateway $GW"; then
    FAILED="Gateway $GW nicht mehr erreichbar nach Stellwerk-Start"
fi

if [[ -n "$SSH_IP" ]]; then
    if ! ping_host "$SSH_IP" "SSH-Client $SSH_IP"; then
        FAILED="SSH-Client $SSH_IP nicht mehr erreichbar – SSH-Verbindung gefährdet"
    fi
fi

if [[ -n "$FAILED" ]]; then
    rollback "$FAILED"
fi

# ---- Alles OK ----
echo ""
echo "==> OK: Stellwerk läuft, Netzwerk erreichbar."
echo "    Service ist NICHT boot-persistent."
echo "    Für Autostart: systemctl enable stellwerk"
echo ""
systemctl status stellwerk --no-pager -l
