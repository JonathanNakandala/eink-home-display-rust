#!/bin/sh
# Before the files are taken away. Not on an upgrade, which is the same package again: Debian says "remove" for a removal and
# "upgrade" for an upgrade, and Fedora says 0 for a removal and 1 for an upgrade.
set -e

case "$1" in
    remove | 0)
        if [ -d /run/systemd/system ]; then
            systemctl disable --now home-display-health.timer home-display.service || true
        fi
        ;;
esac
