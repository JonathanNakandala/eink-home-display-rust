#!/bin/sh
# After the files are put down, on a first install and on an upgrade.
set -e

STATE=/var/lib/home-display
CONFIG=/etc/home-display/config.toml

# Whether this is the first time, decided before anything is put in the folder: an empty one is the only place to make the
# certificate authority, since one that is empty later is most often a volume that was not mounted.
fresh=0
if [ ! -d "$STATE" ] || [ -z "$(ls -A "$STATE")" ]; then fresh=1; fi
install -d -m 0700 -o home-display -g home-display "$STATE"

init=""
if [ "$fresh" -eq 1 ]; then init="--init-pki"; fi
# As the service's user, in its folder, so what the check makes there is its own. A configuration that is wrong is said, and
# does not undo the installation: the service is not started by it, and the next start says the same.
# shellcheck disable=SC2086
if ! runuser -u home-display -- env -C "$STATE" RUST_LOG=warn /usr/bin/home-display-server \
    --config-file "$CONFIG" --check-config $init; then
    echo "home-display-server: the configuration in $CONFIG is not valid: fix it, then restart the service." >&2
fi

if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
    # Only if it is running: an upgrade restarts it on the new program, and a first install starts nothing.
    systemctl try-restart home-display.service || true
fi

if [ "$fresh" -eq 1 ]; then
    cat <<'MESSAGE'

home-display-server is installed and not started.
  1. Edit /etc/home-display/config.toml: at least [location], and your departure boards.
  2. sudo systemctl enable --now home-display home-display-health.timer home-display-backup.timer
  3. sudo home-display-preflight          (what on this machine would stop it: Chrome, the firewall, the clock)
  4. Backups, one a day: to encrypt them, and to copy them to another machine (a disk that fails takes this one's with it),
     edit /etc/home-display/backup.env. Try one: sudo home-display-backup
More: /usr/share/doc/home-display-server/INSTALL.md
MESSAGE
fi
