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
mkdir -p "$INSTALL_DIR/wg"
chown stellwerk:stellwerk "$INSTALL_DIR/wg"
chmod 700 "$INSTALL_DIR/wg"

echo "==> Installiere Binary nach $BINARY_DST"
install -m 755 "$BINARY_SRC" "$BINARY_DST"

echo "==> Entferne File-Capabilities vom Binary (werden via AmbientCapabilities im Service gesetzt)"
# AmbientCapabilities im systemd-Service sorgt dafür, dass ip/nft als Child-Prozesse
# CAP_NET_ADMIN erben. File-Caps auf dem Binary würden die AmbientCaps beim Exec löschen.
setcap -r "$BINARY_DST" 2>/dev/null || true

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

echo "==> Setze Dateiberechtigungen für stellwerk-User"
# rt_tables: stellwerk muss Mullvad-Routing-Tabellen eintragen können
setfacl -m u:stellwerk:rw /etc/iproute2/rt_tables
# /etc/wireguard/: ACL sollte bereits gesetzt sein (für direkten Config-Zugriff)
setfacl -m u:stellwerk:rwx /etc/wireguard/ 2>/dev/null || true

echo "==> Konfiguriere sudo für WireGuard-Helper"
SUDOERS_FILE="/etc/sudoers.d/stellwerk-wg"
echo "stellwerk ALL=(root) NOPASSWD: $INSTALL_DIR/wg-helper.sh" > "$SUDOERS_FILE"
chmod 440 "$SUDOERS_FILE"
echo "    Sudoers-Eintrag geschrieben: $SUDOERS_FILE"

echo "==> Konfiguriere Unbound DNS-Routing"
# Erstelle stellwerk-managed Unbound-Config (stellwerk schreibt outgoing-interface rein)
UNBOUND_STELLWERK_CONF="/etc/unbound/stellwerk.conf"
touch "$UNBOUND_STELLWERK_CONF"
chown stellwerk:stellwerk "$UNBOUND_STELLWERK_CONF"
chmod 644 "$UNBOUND_STELLWERK_CONF"
echo "    $UNBOUND_STELLWERK_CONF erstellt (owner: stellwerk)"

# Füge include-toplevel zu unbound.conf hinzu (einmalig, idempotent)
if ! grep -q "stellwerk.conf" /etc/unbound/unbound.conf; then
    echo "include-toplevel: \"/etc/unbound/stellwerk.conf\"" >> /etc/unbound/unbound.conf
    echo "    include-toplevel für stellwerk.conf in unbound.conf eingetragen"
fi

# sudo-Regel: stellwerk darf unbound neustarten (für outgoing-interface-Änderungen)
SUDOERS_UNBOUND="/etc/sudoers.d/stellwerk-unbound"
echo "stellwerk ALL=(root) NOPASSWD: /usr/bin/systemctl restart unbound" > "$SUDOERS_UNBOUND"
chmod 440 "$SUDOERS_UNBOUND"
echo "    Sudoers-Eintrag geschrieben: $SUDOERS_UNBOUND"

echo "==> Installiere Systemd-Service"
install -m 644 "$SERVICE_SRC" "$SERVICE_DST"
systemctl daemon-reload

echo "==> Starte stellwerk (NICHT boot-persistent – kein enable)"
echo "    Für Autostart: systemctl enable stellwerk"
systemctl start stellwerk

echo ""
echo "Fertig. Status:"
systemctl status stellwerk --no-pager -l || true
