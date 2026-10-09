#!/bin/sh
# Runs a command against a real server: stages the fixture and displayctl (stage_programs.sh), starts the fixture with a fresh authority that
# requires the RFC 9266 channel binding, runs the command with where things are in its environment, and stops the
# fixture. The command's exit status is the result.
#
#   tests/tls/with_fixture.sh <command> [arguments]
#
# In the command's environment: FIXTURE_PORT (HTTPS), FIXTURE_ADMIN (the admin socket) and DISPLAYCTL (the program).
# Set FIXTURE_PROGRAM and DISPLAYCTL_PROGRAM to use programs already built (see below).
#
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
port=${FIXTURE_PORT:-18443}

# The programs come from stage_programs.sh, which `make host-test` runs once and says where with FIXTURE_PROGRAM and
# DISPLAYCTL_PROGRAM. Run on its own this stages them itself: cargo checks for a change (a moment), and the copies are only
# replaced when they differ, so the system does not verify the server's signature again for nothing.
if [ -z "${FIXTURE_PROGRAM:-}" ] || [ -z "${DISPLAYCTL_PROGRAM:-}" ]; then
    staged=$here/../../build/host/bin
    "$here/stage_programs.sh" "$staged"
    FIXTURE_PROGRAM=$(cd "$staged" && pwd)/est_fixture
    DISPLAYCTL_PROGRAM=$(cd "$staged" && pwd)/displayctl
fi
fixture_program=$FIXTURE_PROGRAM
displayctl_program=$DISPLAYCTL_PROGRAM

# mktemp, and not a directory in the repository: a Unix socket's path is limited to about a hundred bytes.
work=$(mktemp -d)/pki
"$fixture_program" "$work" "$port" require-binding >"$work.log" 2>&1 &
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

FIXTURE_PORT=$port FIXTURE_ADMIN="$work/admin.sock" DISPLAYCTL="$displayctl_program" "$@"
