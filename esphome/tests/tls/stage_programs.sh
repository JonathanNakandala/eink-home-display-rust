#!/bin/sh
# Builds the test server (examples/est_fixture) and displayctl once and copies them to the directory given, where they stay
# put: cargo replaces its own copies on every call, even when nothing was built, and macOS then checks the new file's
# signature again before it first runs, which for the 40 MB server is about half a second each time. A copy that is only
# replaced when it differs is checked once.
#
#   tests/tls/stage_programs.sh <directory>
#
# Cargo decides where the programs go (CARGO_TARGET_DIR, `build.target-dir`), so this asks it. A build that fails says why
# and stops here.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
to=$1
mkdir -p "$to"

built=$(cargo build -q --message-format=json-render-diagnostics --manifest-path "$root/Cargo.toml" \
    --example est_fixture --bin displayctl)

for program in est_fixture displayctl; do
    path=$(printf '%s\n' "$built" | grep -o '"executable":"[^"]*/'"$program"'"' | head -n 1 | sed 's/^"executable":"//; s/"$//')
    if [ -z "$path" ] || [ ! -x "$path" ]; then
        echo "cargo did not build $program, or it is not where cargo says (${path:-no path})" >&2
        exit 1
    fi
    cmp -s "$path" "$to/$program" 2>/dev/null || cp "$path" "$to/$program"
done
