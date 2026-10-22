#!/usr/bin/env bash
# Checks that this machine can run the server, before (or after) it is installed, and says what to do about anything that
# cannot. Changes nothing. Run by install.sh, and on its own:   ./preflight.sh [directory with home-display-server in it]
#
# Each line starts with ok, WARN (it may still work, or it works without something) or FAIL (it will not). The exit status is
# non-zero if there is a FAIL.
set -uo pipefail

BIN_DIR="${1:-}"
failed=0
ok() { printf '  ok    %s\n' "$*"; }
warn() { printf '  WARN  %s\n' "$*"; }
fail() { printf '  FAIL  %s\n' "$*"; failed=1; }
have() { command -v "$1" > /dev/null 2>&1; }

echo "Checking this machine"

# --- systemd
if [ -d /run/systemd/system ] && have systemctl; then
    version=$(systemctl --version | awk 'NR==1 {print $2}')
    ok "systemd $version"
    if [ "${version%%[^0-9]*}" -lt 254 ] 2> /dev/null; then
        warn "systemd older than 254 ignores RestartSteps and RestartMaxDelaySec in the unit: a service that keeps failing is tried every 30 s and not less often; set a longer RestartSec in a drop-in"
    fi
else
    fail "this is not running under systemd (no /run/systemd/system): the unit cannot be used"
fi
have systemd-sysusers && ok "systemd-sysusers (makes the service's user)" || fail "systemd-sysusers is missing"
have runuser && ok "runuser" || fail "runuser (util-linux) is missing: the installer checks the configuration as the service's user with it"

# --- the program itself
if [ "$(uname -m)" = x86_64 ]; then
    missing=()
    for flag in avx2 bmi2 fma; do
        grep -qw "$flag" /proc/cpuinfo || missing+=("$flag")
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        fail "this CPU lacks ${missing[*]}, which the release build needs (x86-64-v3, 2013 or later for Intel Core, 2017 for AMD): it would stop with \"illegal instruction\". Build with plain cargo build --release instead"
    else
        ok "CPU has AVX2, BMI2 and FMA"
    fi
else
    warn "this is $(uname -m): the release build is for x86-64 (v3); build with cargo build --release"
fi
if [ -n "$BIN_DIR" ] && [ -x "$BIN_DIR/home-display-server" ]; then
    if "$BIN_DIR/home-display-server" --help > /dev/null 2>&1; then
        ok "home-display-server starts"
    else
        fail "$BIN_DIR/home-display-server does not start (a library it needs, or an instruction this CPU lacks): try ldd on it"
    fi
fi

# --- Chrome, which draws the pictures
chrome=""
for name in google-chrome-stable google-chrome chromium chromium-browser chrome; do
    if have "$name"; then chrome=$(command -v "$name"); break; fi
done
if [ -z "$chrome" ]; then
    warn "no Chrome or Chromium: the server will download one into its cache on the first render (needs the network, and several hundred MB). Better to install one: Google Chrome from google.com/chrome, or on Fedora: dnf install chromium"
elif [[ "$(readlink -f "$chrome")" == */snap/* ]] || { have snap && snap list chromium > /dev/null 2>&1; }; then
    warn "Chromium here is a snap ($chrome), which a service with no login cannot start under the sandbox the unit sets: install Google Chrome from google.com/chrome instead, or give --bundled-chrome in the unit to use the download"
else
    ok "Chrome: $chrome"
fi
# Chrome's own sandbox is made of user namespaces.
userns_clone=$(sysctl -n kernel.unprivileged_userns_clone 2> /dev/null || echo 1)
max_userns=$(sysctl -n user.max_user_namespaces 2> /dev/null || echo 1)
apparmor_userns=$(sysctl -n kernel.apparmor_restrict_unprivileged_userns 2> /dev/null || echo 0)
if [ "$userns_clone" = 0 ] || [ "$max_userns" = 0 ]; then
    warn "unprivileged user namespaces are off (kernel.unprivileged_userns_clone=$userns_clone, user.max_user_namespaces=$max_userns): Chrome's sandbox will not start"
elif [ "$apparmor_userns" = 1 ]; then
    warn "AppArmor restricts unprivileged user namespaces (kernel.apparmor_restrict_unprivileged_userns=1, Ubuntu 23.10 and later): Chrome may fail to start. Google Chrome's package ships a profile that allows it; otherwise see deploy/README.md, \"Sandboxing\""
else
    ok "user namespaces are allowed"
fi
if have getenforce && [ "$(getenforce 2> /dev/null)" = Enforcing ]; then
    warn "SELinux is enforcing: if Chrome fails to start under the service, look in 'ausearch -m avc -ts recent' (INSTALL.md says what to do)"
fi

# --- the clock, which the schedule and the certificates are read against
if have timedatectl; then
    if [ "$(timedatectl show -p NTPSynchronized --value 2> /dev/null)" = yes ]; then
        ok "the clock is synchronised"
    else
        warn "the clock is not synchronised: enable a time service (timedatectl set-ntp true, or chrony), since the schedule and the certificates depend on it"
    fi
fi
have curl && ok "curl (for the health check)" || warn "curl is missing: the health check needs it"

# --- the network, unless the server is already running it
if systemctl is-active --quiet home-display.service 2> /dev/null; then
    ok "home-display is running (its ports are not checked)"
elif have ss; then
    for port in 8080 8443; do
        if ss -H -ltn "sport = :$port" 2> /dev/null | grep -q .; then
            warn "something already listens on port $port: change [server] bind or [server.tls] bind"
        else
            ok "port $port is free"
        fi
    done
fi
if have ss && ss -H -lun "sport = :5353" 2> /dev/null | grep -q .; then
    ok "something else answers mDNS on 5353 (Avahi or systemd-resolved): the server shares the port, so check the displays find it after the first start"
fi
if have firewall-cmd && firewall-cmd --state > /dev/null 2>&1; then
    warn "firewalld is running: allow the displays in (sudo firewall-cmd --permanent --add-port=8080/tcp --add-port=8443/tcp --add-service=mdns && sudo firewall-cmd --reload)"
elif have ufw && ufw status 2> /dev/null | grep -q "Status: active"; then
    warn "ufw is active: allow the displays in (sudo ufw allow from 192.168.0.0/16 to any port 8080,8443 proto tcp, and sudo ufw allow 5353/udp), with your own network"
fi

echo
if [ "$failed" -ne 0 ]; then
    echo "At least one check failed."
else
    echo "Nothing stops it."
fi
exit "$failed"
