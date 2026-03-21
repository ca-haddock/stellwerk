#!/bin/bash
# Stellwerk Install-Script
# Muss als root ausgeführt werden.

set -euo pipefail

BINARY_SRC="target/release/stellwerk"
INSTALL_DIR="/home/stellwerk"
BINARY_DST="$INSTALL_DIR/bin/stellwerk"
CONFIG_SRC="config/default.toml"
CONFIG_DST="$INSTALL_DIR/config.toml"
SERVICE_SRC="stellwerk.service"
SERVICE_DST="/etc/systemd/system/stellwerk.service"

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

echo "==> Erstelle System-User 'stellwerk'"
if ! id stellwerk &>/dev/null; then
    useradd --system --home-dir "$INSTALL_DIR" --create-home --shell /sbin/nologin stellwerk
    echo "    User 'stellwerk' angelegt"
else
    echo "    User 'stellwerk' existiert bereits"
fi

echo "==> Erstelle Verzeichnisse"
mkdir -p "$INSTALL_DIR/bin"

echo "==> Installiere Binary nach $BINARY_DST"
install -m 755 "$BINARY_SRC" "$BINARY_DST"

echo "==> Setze Linux Capabilities (CAP_NET_ADMIN) auf Binary"
# Ersetzt root-Rechte: nft und ip rule brauchen nur CAP_NET_ADMIN
setcap cap_net_admin+eip "$BINARY_DST"

if [[ -f "$CONFIG_DST" ]]; then
    echo "==> Config existiert bereits, überspringe: $CONFIG_DST"
else
    echo "==> Kopiere Default-Config nach $CONFIG_DST"
    install -m 640 "$CONFIG_SRC" "$CONFIG_DST"
    echo ""
    echo "    !! Config bitte anpassen: $CONFIG_DST"
    echo "       - TLS cert/key Pfade prüfen"
    echo "       - Auth Passwort-Hash setzen:"
    echo "         echo -n 'meinpasswort' | sha256sum | awk '{print \$1}'"
    echo "       - HA-Token, InfluxDB etc."
    echo ""
fi

echo "==> Setze Besitzer für $INSTALL_DIR"
chown -R stellwerk:stellwerk "$INSTALL_DIR"

echo "==> Installiere Systemd-Service"
install -m 644 "$SERVICE_SRC" "$SERVICE_DST"
systemctl daemon-reload

echo "==> Aktiviere und starte stellwerk"
systemctl enable --now stellwerk

echo ""
echo "Fertig. Status:"
systemctl status stellwerk --no-pager -l || true
