#!/usr/bin/env bash
# Installs, or upgrades, the home display server on a machine that runs systemd (Ubuntu, Debian, Fedora, and the like).
#
#   sudo ./install.sh                installs from the folder the release archive was unpacked into
#   sudo ./install.sh --uninstall    removes the program and the units, and leaves the configuration and the data
#
# Safe to run again: an upgrade is the same command from the newer archive. It never replaces /etc/home-display/config.toml
# or anything in /var/lib/home-display, and it makes the certificate authority (when the configuration uses one) only on a
# machine whose /var/lib/home-display is empty, since a directory that is empty later is a volume that was not mounted, and a
# new authority would lock every paired display out.
#
# Options:  --no-start         install, but don't start (or restart) the service
#           --skip-preflight   don't check the machine first (preflight.sh says what it looks for)
#           --bin-dir DIR      where home-display-server and displayctl are (default: beside this script, or ../target/release)
set -euo pipefail

SERVICE_USER=home-display
BIN_TARGET=/usr/local/bin
CONFIG_DIR=/etc/home-display
CONFIG="$CONFIG_DIR/config.toml"
STATE_DIR=/var/lib/home-display
UNIT_DIR=/etc/systemd/system

start=1
preflight=1
uninstall=0
bin_dir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-start) start=0 ;;
        --skip-preflight) preflight=0 ;;
        --uninstall) uninstall=1 ;;
        --bin-dir) bin_dir="${2:?--bin-dir needs a folder}"; shift ;;
        -h | --help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "install.sh: unknown option $1 (see --help)" >&2; exit 2 ;;
    esac
    shift
done

say() { printf '\n== %s\n' "$*"; }
die() { echo "install.sh: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run it as root: sudo $0"

# A .deb or .rpm owns /usr/bin, /usr/lib/systemd/system and the same user and state: an install here beside it would leave two
# copies of the program and two sets of units, and the one in /etc/systemd/system would win. Upgrade with the package manager.
if dpkg-query --show --showformat='${db:Status-Status}' home-display-server 2> /dev/null | grep -qx installed; then
    die "home-display-server is installed as a .deb. Upgrade it with apt (apt install ./home-display-server_*.deb), or remove it first: apt remove home-display-server"
elif rpm --query home-display-server > /dev/null 2>&1; then
    die "home-display-server is installed as an .rpm. Upgrade it with dnf (dnf upgrade ./home-display-server-*.rpm), or remove it first: dnf remove home-display-server"
fi

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# In the release archive the units and templates are in deploy/ beside this script; in a checkout they are beside it.
if [ -d "$HERE/deploy" ]; then DEPLOY="$HERE/deploy"; else DEPLOY="$HERE"; fi

if [ "$uninstall" -eq 1 ]; then
    say "Removing the program and the units"
    systemctl disable --now home-display-backup.timer home-display-health.timer home-display.service 2> /dev/null || true
    rm -f "$UNIT_DIR/home-display.service" "$UNIT_DIR/home-display-health.service" "$UNIT_DIR/home-display-health.timer" \
        "$UNIT_DIR/home-display-backup.service" "$UNIT_DIR/home-display-backup.timer" \
        "$BIN_TARGET/home-display-server" "$BIN_TARGET/displayctl" "$BIN_TARGET/home-display-health-check" \
        "$BIN_TARGET/home-display-backup" "$BIN_TARGET/home-display-restore" \
        /etc/sysusers.d/home-display.conf
    systemctl daemon-reload
    echo "Kept: $CONFIG_DIR and $STATE_DIR (the certificate authority, the displays, the keys), the backups in"
    echo "/var/backups/home-display, and the user $SERVICE_USER."
    echo "To remove them too:  rm -r $CONFIG_DIR $STATE_DIR /var/backups/home-display && userdel $SERVICE_USER"
    exit 0
fi

command -v systemctl > /dev/null && [ -d /run/systemd/system ] || die "this machine is not running systemd"

if [ -z "$bin_dir" ]; then
    if [ -x "$HERE/home-display-server" ]; then bin_dir="$HERE"
    elif [ -x "$HERE/../target/release/home-display-server" ]; then bin_dir="$HERE/../target/release"
    else die "home-display-server is not beside this script or in ../target/release: unpack the release archive and run install.sh from it, or give --bin-dir"
    fi
fi
for file in "$bin_dir/home-display-server" "$bin_dir/displayctl" "$DEPLOY/home-display.service" "$DEPLOY/home-display-health.service" \
    "$DEPLOY/home-display-health.timer" "$DEPLOY/health-check.sh" "$DEPLOY/home-display.sysusers" "$DEPLOY/config.production.toml" \
    "$DEPLOY/home-display-backup.service" "$DEPLOY/home-display-backup.timer" "$DEPLOY/backup.sh" "$DEPLOY/restore.sh" \
    "$DEPLOY/backup.env"; do
    [ -r "$file" ] || die "$file is missing"
done

if [ "$preflight" -eq 1 ]; then
    say "Checking the machine"
    bash "$DEPLOY/preflight.sh" "$bin_dir" || die "fix what failed above, or run with --skip-preflight if you know better"
fi

say "The user and the folders"
install -D -m 0644 "$DEPLOY/home-display.sysusers" /etc/sysusers.d/home-display.conf
systemd-sysusers /etc/sysusers.d/home-display.conf
# Whether this is the first time, decided before anything is put in the folder.
fresh=0
if [ ! -d "$STATE_DIR" ] || [ -z "$(ls -A "$STATE_DIR")" ]; then fresh=1; fi
install -d -m 0750 -o root -g "$SERVICE_USER" "$CONFIG_DIR"
install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_USER" "$STATE_DIR"

say "The program"
# Both in /usr/local/bin: the place SELinux already labels as programs, so a service may run them on Fedora too.
install -m 0755 "$bin_dir/home-display-server" "$bin_dir/displayctl" "$BIN_TARGET/"
install -m 0755 "$DEPLOY/health-check.sh" "$BIN_TARGET/home-display-health-check"
install -m 0755 "$DEPLOY/backup.sh" "$BIN_TARGET/home-display-backup"
install -m 0755 "$DEPLOY/restore.sh" "$BIN_TARGET/home-display-restore"
if command -v restorecon > /dev/null; then
    restorecon -F "$BIN_TARGET/home-display-server" "$BIN_TARGET/displayctl" "$BIN_TARGET/home-display-health-check" \
        "$BIN_TARGET/home-display-backup" "$BIN_TARGET/home-display-restore" 2> /dev/null || true
fi

say "The configuration"
if [ -e "$CONFIG" ]; then
    echo "Kept $CONFIG as it is. The current template is $DEPLOY/config.production.toml, to compare."
else
    install -m 0640 -o root -g "$SERVICE_USER" "$DEPLOY/config.production.toml" "$CONFIG"
    echo "Made $CONFIG. Edit [location] (and add your departure boards) before the displays rely on it."
fi
# The backup's settings: root only (they may name a key), and never replaced, like the configuration.
if [ ! -e "$CONFIG_DIR/backup.env" ]; then
    install -m 0600 -o root -g root "$DEPLOY/backup.env" "$CONFIG_DIR/backup.env"
fi
# As the user the service runs as, in its folder, so what the check makes there is the service's own. A configuration that
# is wrong stops the installation here, with the program's own words, and not at a start that then fails in the journal.
check=(--config-file "$CONFIG" --check-config)
if [ "$fresh" -eq 1 ]; then check+=(--init-pki); fi
runuser -u "$SERVICE_USER" -- env -C "$STATE_DIR" RUST_LOG=warn "$BIN_TARGET/home-display-server" "${check[@]}" \
    || die "the configuration in $CONFIG is not valid: fix it and run install.sh again"
if [ "$fresh" -eq 1 ]; then
    echo "(First install: the certificate authority was made, if the configuration uses one. The daily backup keeps it safe: see below.)"
fi

say "The service"
install -m 0644 "$DEPLOY/home-display.service" "$DEPLOY/home-display-health.service" "$DEPLOY/home-display-health.timer" \
    "$DEPLOY/home-display-backup.service" "$DEPLOY/home-display-backup.timer" "$UNIT_DIR/"
systemctl daemon-reload
systemctl enable home-display.service home-display-health.timer home-display-backup.timer
if [ "$start" -eq 1 ]; then
    # Starts it the first time and restarts it on an upgrade, so the new program is the one running.
    systemctl restart home-display.service
    systemctl start home-display-health.timer home-display-backup.timer
else
    echo "Enabled, not started (--no-start): systemctl start home-display"
fi

say "Done"
if [ "$start" -eq 1 ]; then
    sleep 2
    systemctl --no-pager --lines=0 status home-display.service || true
fi
cat <<MESSAGE

Next:
  Watch it:        journalctl -u home-display -f
  Is it working:   curl http://localhost:8080/healthz        (ok, or what is wrong)
  Keys for the weather and the departures, if they need them:
                   sudo -u $SERVICE_USER displayctl -c $CONFIG credential set --help
  Let a display join (HTTPS):
                   sudo -u $SERVICE_USER displayctl -c $CONFIG window open
  Backups:         one a day is kept in /var/backups/home-display (two weeks of them). To encrypt them, and to copy them to
                   another machine, which a disk that fails needs: /etc/home-display/backup.env. Try one now: sudo home-display-backup
  More:            INSTALL.md, and deploy/README.md for everything the server does.
MESSAGE
