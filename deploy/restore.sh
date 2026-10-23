#!/usr/bin/env bash
# Puts a backup made by home-display-backup back where it was. Installed as home-display-restore.
#
#   home-display-restore ARCHIVE                  restore it
#   home-display-restore --list ARCHIVE           say what is in it, and change nothing
#   home-display-restore --identity KEY ARCHIVE   for an encrypted one (.age): the age private key file that opens it
#
# Run as root, with the service stopped (systemctl stop home-display). It checks the archive first (its checksum if there is a
# .sha256 beside it, and that it holds only the authority's folder and the configuration), puts what is there now aside, as
# NAME.before-restore-TIME and never deletes it, and then extracts. The user the files belong to must exist: on a new machine,
# install the package (or run install.sh) first, then restore, then start the service.
set -euo pipefail

# Where things are. Not normally set: they are here so the tests can use a folder of their own.
STATE_DIR="${BACKUP_STATE_DIR:-/var/lib/home-display}"
CONFIG_FILE="${BACKUP_CONFIG_FILE:-/etc/home-display/config.toml}"
ROOT="${RESTORE_ROOT:-/}"

die() { echo "home-display-restore: $*" >&2; exit 1; }
log() { echo "home-display-restore: $*"; }

list_only=0
identity="${AGE_IDENTITY:-}"
archive=""
while [ $# -gt 0 ]; do
    case "$1" in
        --list) list_only=1 ;;
        --identity) identity="${2:?--identity needs the key file}"; shift ;;
        -h | --help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) die "unknown option $1 (see --help)" ;;
        *) [ -z "$archive" ] || die "one archive at a time"; archive="$1" ;;
    esac
    shift
done
[ -n "$archive" ] || die "which archive? (see --help)"
[ -f "$archive" ] || die "$archive is not a file"
[ "$list_only" -eq 1 ] || [ "$(id -u)" -eq 0 ] || die "run it as root"

# Open it: through age first if it is encrypted. A stream, so the plaintext is never in a file.
open_archive() {
    case "$archive" in
        *.age)
            [ -n "$identity" ] || die "$archive is encrypted: give the key that opens it with --identity FILE"
            [ -r "$identity" ] || die "the key $identity cannot be read"
            command -v age > /dev/null || die "age is not installed (apt install age, or dnf install age)"
            age --decrypt --identity "$identity" -- "$archive" | gzip --decompress --stdout
            ;;
        *) gzip --decompress --stdout < "$archive" ;;
    esac
}

# Only what home-display-backup makes: the authority's folder and the configuration, under the paths it keeps them at.
allowed_pki="${STATE_DIR#/}/pki"
allowed_config="${CONFIG_FILE#/}"
contents=$(open_archive | tar --list) || die "$archive cannot be read: wrong key, or not a backup?"
[ -n "$contents" ] || die "$archive is empty"
while IFS= read -r path; do
    path="${path%/}"
    case "$path" in
        /* | *..*) die "$archive has a path that is not safe to restore: $path" ;;
    esac
    case "$path" in
        "$allowed_pki" | "$allowed_pki"/* | "$allowed_config") ;;
        *) die "$archive has $path, which is not the authority's folder or the configuration: not restoring it" ;;
    esac
done <<< "$contents"
printf '%s\n' "$contents" | grep -c "^$allowed_pki/" > /dev/null || die "$archive has nothing of the authority in it"

if [ "$list_only" -eq 1 ]; then
    printf '%s\n' "$contents"
    exit 0
fi

# The checksum, if one was kept beside it: what was copied off this machine arrived whole.
if [ -f "$archive.sha256" ]; then
    (cd "$(dirname "$archive")" && sha256sum --check --status "$(basename "$archive").sha256") \
        || die "$archive does not match its checksum ($archive.sha256): it was damaged, or changed"
    log "checksum good"
fi

if [ "$ROOT" = / ] && systemctl is-active --quiet home-display.service 2> /dev/null; then
    die "the service is running: stop it first (systemctl stop home-display), then restore"
fi

stamp=$(date -u +%Y%m%dT%H%M%SZ)
for existing in "$ROOT/$allowed_pki" "$ROOT/$allowed_config"; do
    if [ -e "$existing" ]; then
        mv -- "$existing" "$existing.before-restore-$stamp"
        log "kept what was there as $existing.before-restore-$stamp"
    fi
done

mkdir -p "$ROOT"
open_archive | tar --extract --preserve-permissions --directory="$ROOT" --file=-
log "restored $archive"

if [ "$ROOT" = / ] && command -v home-display-server > /dev/null && id home-display > /dev/null 2>&1; then
    # As the service's user, in its folder, as the installer checks it: the restored files are read as the server reads them.
    if runuser -u home-display -- env -C "$STATE_DIR" RUST_LOG=warn home-display-server --config-file "$CONFIG_FILE" --check-config; then
        log "the server reads them: start it with systemctl start home-display"
    else
        log "WARNING: the server does not accept what was restored (above). The old files are beside them, named .before-restore-$stamp."
    fi
else
    log "start the service when you are ready: systemctl start home-display"
fi
