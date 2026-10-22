#!/bin/sh
# Before the files are put down: refuse a machine the program cannot run on, and make the user the files are given to.
# Plain sh, because this runs under dash on Debian and bash on Fedora.
set -e

for flag in avx2 bmi2 fma; do
    if ! grep -qw "$flag" /proc/cpuinfo; then
        echo "home-display-server: this CPU lacks $flag, which the build needs (x86-64-v3: Intel Core from 2013, AMD from 2017)." >&2
        echo "Build it yourself with cargo build --release instead." >&2
        exit 1
    fi
done

# The same line as /usr/lib/sysusers.d/home-display.conf, which is not there yet. Does nothing if the user exists.
systemd-sysusers --inline 'u home-display - "Home display server" /var/lib/home-display /usr/sbin/nologin'
