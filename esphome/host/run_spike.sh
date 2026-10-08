#!/bin/sh
# Runs the TLS spike against a real server: builds the fixture and displayctl, starts the fixture with a fresh authority
# that requires the RFC 9266 channel binding, runs spike_tls against it, and stops it. Exit status is the spike's.
#   host/run_spike.sh [port]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
port=${1:-18443}

cargo build -q --manifest-path "$root/Cargo.toml" --example est_fixture --bin displayctl
cmake --build "$root/esphome/build/host" --target spike_tls >/dev/null

# mktemp, and not a directory in the repository: a Unix socket's path is limited to about a hundred bytes.
work=$(mktemp -d)/pki
"$root/target/debug/examples/est_fixture" "$work" "$port" require-binding >"$work.log" 2>&1 &
fixture=$!
trap 'kill $fixture 2>/dev/null || true' EXIT

# Wait for READY.
i=0
until grep -q '^READY' "$work.log" 2>/dev/null; do
    i=$((i + 1))
    if [ $i -gt 100 ] || ! kill -0 $fixture 2>/dev/null; then
        echo "the fixture did not start:" >&2
        cat "$work.log" >&2
        exit 1
    fi
    sleep 0.1
done

"$root/esphome/build/host/spike_tls" "$port" "$root/target/debug/displayctl" "$work/admin.sock" reterminal-e1003-a1b2c3
