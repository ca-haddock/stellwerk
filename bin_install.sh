#!/bin/bash
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "Fehler: root erforderlich." >&2
    exit 1
fi

# Version in Cargo.toml hochzählen (Patch-Level)
CURRENT=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"
PATCH=$(( PATCH + 1 ))
NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"

# Bei Build-Fehler: Version zurücksetzen
restore_version() {
    echo "Build fehlgeschlagen – Version zurückgesetzt auf ${CURRENT}" >&2
    sed -i "s/^version = \"${NEW_VERSION}\"/version = \"${CURRENT}\"/" Cargo.toml
}
trap restore_version ERR

sed -i "s/^version = \"${CURRENT}\"/version = \"${NEW_VERSION}\"/" Cargo.toml
echo "Version: ${CURRENT} → ${NEW_VERSION}"

# Release-Build
sudo -u claude /home/claude/.cargo/bin/cargo build --release

# Build erfolgreich – Trap aufheben
trap - ERR

install -m 755 target/release/stellwerk /home/stellwerk/bin/stellwerk
# Keine File-Capabilities – AmbientCapabilities im systemd-Service übernimmt das.
# (File-Caps auf dem Binary löschen die AmbientCaps beim Exec → Child-Prozesse erben nichts)
setcap -r /home/stellwerk/bin/stellwerk 2>/dev/null || true
systemctl restart stellwerk
systemctl status stellwerk --no-pager -l
