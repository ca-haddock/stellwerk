#!/bin/bash
# Stellwerk Install-Script
# Muss als root ausgeführt werden.

set -euo pipefail

BINARY_SRC="target/release/stellwerk"
BINARY_DST="/usr/local/bin/stellwerk"
CONFIG_SRC="config/default.toml"
CONFIG_DST="/etc/stellwerk/config.toml"
SERVICE_SRC="stellwerk.service"
SERVICE_DST="/etc/systemd/system/stellwerk.service"
CONFIG_DIR="/etc/stellwerk"
DATA_DIR="/var/lib/stellwerk"

# --- Checks ---

if [[ $EUID -ne 0 ]]; then
    echo "Fehler: Dieses Script muss als root ausgeführt werden." >&2
    exit 1
fi

if [[ ! -f "$BINARY_SRC" ]]; then
    echo "Binary nicht gefunden: $BINARY_SRC"
    echo "Bitte zuerst: cargo build --release"
    exit 1
fi

# --- Install ---

echo "==> Installiere Binary nach $BINARY_DST"
install -m 755 "$BINARY_SRC" "$BINARY_DST"

echo "==> Erstelle Verzeichnisse"
mkdir -p "$CONFIG_DIR" "$DATA_DIR"

if [[ -f "$CONFIG_DST" ]]; then
    echo "==> Config existiert bereits, überspringe: $CONFIG_DST"
else
    echo "==> Kopiere Default-Config nach $CONFIG_DST"
    install -m 640 "$CONFIG_SRC" "$CONFIG_DST"
    echo ""
    echo "    !! Config bitte anpassen: $CONFIG_DST"
    echo "       (HA-Token, InfluxDB, Subnets etc.)"
    echo ""
fi

echo "==> Installiere Systemd-Service"
install -m 644 "$SERVICE_SRC" "$SERVICE_DST"
systemctl daemon-reload

echo "==> Aktiviere und starte stellwerk"
systemctl enable --now stellwerk

echo ""
echo "Fertig. Status:"
systemctl status stellwerk --no-pager -l || true
