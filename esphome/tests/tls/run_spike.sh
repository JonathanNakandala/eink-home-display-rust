#!/bin/sh
# Runs the TLS spike (spike_tls.cpp) against a real server. Exit status is the spike's.
#   tests/tls/run_spike.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/with_fixture.sh" sh -c '"$0/../../build/host/spike_tls" "$FIXTURE_PORT" "$DISPLAYCTL" "$FIXTURE_ADMIN" reterminal-e1003-a1b2c3' "$here"
