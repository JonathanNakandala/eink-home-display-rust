#!/bin/bash
# Restarts the service when it is running but not working: /healthz says `stale` (a scheduled render is long overdue), or does not
# answer at all. Run every few minutes by home-display-health.timer, as root, and installed as
# /usr/local/bin/home-display-health-check.
#
# systemd already restarts a service that exits. This is for the one that does not: a process that is up and stuck. It acts only
# after FAILS_BEFORE_RESTART checks in a row have failed, so one slow answer or a render that is merely late does nothing, and
# /healthz itself allows `stale_grace_seconds` (300) after a start before it calls a service stale.
#
# Settings, optional, in /etc/home-display/health.env:
#   HEALTH_URL=          where to look; empty turns the check off (a server whose `transport` is "https" serves /healthz only to
#                        a display's certificate, so a check from here cannot see it)
#   FAILS_BEFORE_RESTART=3
set -euo pipefail

UNIT=home-display.service
HEALTH_URL="http://127.0.0.1:8080/healthz"
FAILS_BEFORE_RESTART=3
# shellcheck disable=SC1091
[ -r /etc/home-display/health.env ] && . /etc/home-display/health.env

STATE="${RUNTIME_DIRECTORY:-/run/home-display-health}/failures"
mkdir -p "$(dirname "$STATE")"

[ -n "$HEALTH_URL" ] || exit 0
# Not running is systemd's to deal with (Restart=on-failure, with its backoff), and restarting it here would undo the backoff.
systemctl is-active --quiet "$UNIT" || { rm -f "$STATE"; exit 0; }

if body=$(curl --silent --show-error --fail --max-time 10 "$HEALTH_URL" 2>&1); then
    rm -f "$STATE"
    exit 0
fi

fails=$(( $(cat "$STATE" 2>/dev/null || echo 0) + 1 ))
echo "$fails" > "$STATE"
echo "health check failed ($fails of $FAILS_BEFORE_RESTART): $body" >&2
if [ "$fails" -ge "$FAILS_BEFORE_RESTART" ]; then
    echo "restarting $UNIT" >&2
    rm -f "$STATE"
    systemctl restart "$UNIT"
fi
