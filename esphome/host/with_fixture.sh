#!/bin/sh
# Runs a command against a real server: builds the fixture and displayctl, starts the fixture with a fresh authority that
# requires the RFC 9266 channel binding, runs the command with where things are in its environment, and stops the
# fixture. The command's exit status is the result.
#
#   host/with_fixture.sh <command> [arguments]
#
# In the command's environment: FIXTURE_PORT (HTTPS), FIXTURE_ADMIN (the admin socket) and DISPLAYCTL (the program).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
port=${FIXTURE_PORT:-18443}

cargo build -q --manifest-path "$root/Cargo.toml" --example est_fixture --bin displayctl

# mktemp, and not a directory in the repository: a Unix socket's path is limited to about a hundred bytes.
work=$(mktemp -d)/pki
"$root/target/debug/examples/est_fixture" "$work" "$port" require-binding >"$work.log" 2>&1 &
fixture=$!
trap 'kill $fixture 2>/dev/null || true' EXIT

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

FIXTURE_PORT=$port FIXTURE_ADMIN="$work/admin.sock" DISPLAYCTL="$root/target/debug/displayctl" "$@"
