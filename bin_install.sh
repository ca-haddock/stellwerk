#!/bin/bash
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "Fehler: root erforderlich." >&2
    exit 1
fi

install -m 755 target/release/stellwerk /home/stellwerk/bin/stellwerk
setcap cap_net_admin+eip /home/stellwerk/bin/stellwerk
systemctl restart stellwerk
systemctl status stellwerk --no-pager -l
