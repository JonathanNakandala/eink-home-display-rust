#!/usr/bin/env bash
# Makes dist/home-display-server_<version>_amd64.deb and dist/home-display-server-<version>-1.x86_64.rpm, with their checksums.
# Run from the repository root after `cargo build --release`, with nfpm installed. Usage: build-packages.sh <version>   (e.g. 0.1.1)
set -euo pipefail

version="${1:?usage: build-packages.sh <version>}"
stage=dist/pkg

rm -rf "$stage"
mkdir -p "$stage"
# -s: stripped of its symbol table, as a package's programs are (a panic still says what and where; only a backtrace's names go).
for bin in home-display-server displayctl; do
    install -m 0755 -s "target/release/${bin}" "$stage/"
done
# The changelog release-please writes, which a package carries as changelog.gz. A build that is not from a release has none yet.
if [ -f CHANGELOG.md ]; then
    gzip -9n -c CHANGELOG.md > "$stage/changelog.gz"
else
    echo 'No changelog: this was not built from a release.' | gzip -9n > "$stage/changelog.gz"
fi
# A deb with a revision (0.2.0-1) carries the upstream changelog as changelog.gz and a Debian one, which says only that there is nothing
# of Debian's to add, as changelog.Debian.gz, in the format Debian reads.
{
    printf 'home-display-server (%s-1) unstable; urgency=medium

' "$version"
    printf '  * Release %s. What changed is in changelog.gz, in this folder.

' "$version"
    printf ' -- home-display-server contributors <JonathanNakandala@users.noreply.github.com>  %s
' "$(date -R)"
} | gzip -9n > "$stage/changelog.Debian.gz"
install -m 0755 deploy/health-check.sh "$stage/home-display-health-check"
install -m 0755 deploy/preflight.sh "$stage/home-display-preflight"
install -m 0755 deploy/backup.sh "$stage/home-display-backup"
install -m 0755 deploy/restore.sh "$stage/home-display-restore"
# A package's programs are in /usr/bin; /usr/local/bin is for what an administrator installs by hand (install.sh).
for unit in home-display.service home-display-health.service home-display-backup.service; do
    sed 's#/usr/local/bin#/usr/bin#g' "deploy/${unit}" > "$stage/${unit}"
done
cp deploy/home-display-health.timer deploy/home-display-backup.timer "$stage/"

for packager in deb rpm; do
    VERSION="$version" nfpm package --config deploy/packaging/nfpm.yaml --packager "$packager" --target dist/
done
(cd dist && for package in *.deb *.rpm; do sha256sum "$package" > "${package}.sha256"; done)
ls -l dist
