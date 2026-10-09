#!/bin/sh
# Runs a command against a real server: builds the fixture and displayctl, starts the fixture with a fresh authority that
# requires the RFC 9266 channel binding, runs the command with where things are in its environment, and stops the
# fixture. The command's exit status is the result.
#
#   tests/tls/with_fixture.sh <command> [arguments]
#
# In the command's environment: FIXTURE_PORT (HTTPS), FIXTURE_ADMIN (the admin socket) and DISPLAYCTL (the program).
#
# Cargo decides where the programs go (CARGO_TARGET_DIR, `build.target-dir` in a config file, a default target), so this asks it
# where it put them and does not assume `target/` in the repository.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
port=${FIXTURE_PORT:-18443}

# The artifacts, as JSON on standard output; the compiler's own messages, rendered for a person, still go to standard error, so a
# build that fails says why. A build that fails stops here (set -e).
built=$(cargo build -q --message-format=json-render-diagnostics --manifest-path "$root/Cargo.toml" \
    --example est_fixture --bin displayctl)

# The path of the program called `$1` that was just built (or found up to date).
built_program() {
    path=$(printf '%s\n' "$built" | grep -o '"executable":"[^"]*/'"$1"'"' | head -n 1 | sed 's/^"executable":"//; s/"$//')
    if [ -z "$path" ] || [ ! -x "$path" ]; then
        echo "cargo did not build $1, or it is not where cargo says (${path:-no path})" >&2
        exit 1
    fi
    printf '%s\n' "$path"
}
fixture_program=$(built_program est_fixture)
displayctl_program=$(built_program displayctl)

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
