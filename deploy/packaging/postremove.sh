#!/bin/sh
# The configuration, the data and the user stay, whatever is removed: the certificate authority in /var/lib/home-display cannot
# be made again, and every display is paired to it.
set -e

if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
fi
