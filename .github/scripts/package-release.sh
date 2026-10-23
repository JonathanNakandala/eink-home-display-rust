#!/usr/bin/env bash
# Packs the release binaries into dist/home-display-server-<version>-x86_64-v3-linux.tar.gz, with its checksum.
# Run from the repository root after `cargo build --release`. Usage: package-release.sh <version>   (e.g. 0.1.1)
set -euo pipefail

version="${1:?usage: package-release.sh <version>}"
name="home-display-server-${version}-x86_64-v3-linux"
bins=(home-display-server displayctl)

rm -rf dist
mkdir -p "dist/${name}"
# -s: stripped of its symbol table, like a package's programs.
for bin in "${bins[@]}"; do
    install -m 0755 -s "target/release/${bin}" "dist/${name}/"
done
cp -r deploy config/example.toml config/quiet_times.toml "dist/${name}/"
# The installer and its guide at the top, where someone who has unpacked the archive looks first. install.sh finds the rest in deploy/.
install -m 0755 deploy/install.sh "dist/${name}/install.sh"
cp deploy/INSTALL.md "dist/${name}/INSTALL.md"
# The licences, which ask to go with every copy: ours, the choice of two, and everything the program contains (made by
# third-party-licences.sh, which has to run first).
cp LICENSE-MIT LICENSE-APACHE THIRD_PARTY_LICENSES.txt "dist/${name}/"

# The archive says root, not whoever the runner was, and the same input gives the same bytes: sorted entries, and the
# time of the commit instead of now.
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$(git log -1 --format=%ct)" \
    -czf "dist/${name}.tar.gz" -C dist "${name}"
(cd dist && sha256sum "${name}.tar.gz" > "${name}.tar.gz.sha256")

ls -l dist
