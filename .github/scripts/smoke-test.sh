#!/usr/bin/env bash
# Catches a binary that was built but cannot start (a missing shared library, or an instruction this CPU lacks) before it
# is published. Run from the repository root after `cargo build --release`.
set -euo pipefail

for bin in home-display-server displayctl; do
    "target/release/${bin}" --help > /dev/null
    echo "${bin}: starts"
done
# What it needs from the host, so a library that is not on a minimal server shows in the log.
ldd target/release/home-display-server
