#!/bin/sh
# Runs every test of the host test programs, each against its own fresh server (with_fixture.sh), the programs side by
# side. A test is run alone with a server of its own because the server limits how many different display names one
# address may join with in a day; the programs are independent of each other, so each takes a port of its own and they
# overlap. A program's results are kept together and printed in order when all are done. The status is 1 if any test failed.
#
#   tests/tls/run_host_tests.sh <build directory> <program>...
#
# In the environment: FIXTURE_PROGRAM and DISPLAYCTL_PROGRAM (stage_programs.sh), and FIXTURE_PORT, the first port to use.
set -u
build=$1
shift
port=${FIXTURE_PORT:-18443}
here=$(cd "$(dirname "$0")" && pwd)

programs="$*"
index=0
pids=""
for binary in "$@"; do
    (
        FIXTURE_PORT=$((port + index))
        export FIXTURE_PORT
        failed=0
        tests=$("$build/$binary" --list) || {
            echo "FAIL $binary (cannot list its tests)"
            exit 1
        }
        for test in $tests; do
            if "$here/with_fixture.sh" "$build/$binary" "$test" >"$build/$test.log" 2>&1; then
                echo "ok   $test"
            else
                echo "FAIL $test"
                cat "$build/$test.log"
                failed=1
            fi
        done
        exit $failed
    ) >"$build/$binary.results" 2>&1 &
    pids="$pids $!"
    index=$((index + 1))
done

failed=0
set -- $pids
for binary in $programs; do
    wait "$1" || failed=1
    shift
    cat "$build/$binary.results"
done
exit $failed
