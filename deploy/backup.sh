#!/usr/bin/env bash
# Backs up what cannot be made again: the certificate authority (its root key above all), the list of displays that have
# joined, the keys kept with --remember, and the configuration. Installed as home-display-backup, and run every day by
# home-display-backup.timer; run it by hand as root the same way. Restoring is home-display-restore.
#
# What it makes: BACKUP_DIR/home-display-<time>-<host>.tar.gz (or .tar.gz.age, see below) and a .sha256 beside it, readable by
# root alone. The pictures and the cache are not in it: they are made again by the next render.
#
# Settings are in /etc/home-display/backup.env (see the example there), all optional:
#   BACKUP_KEEP=14                  how many backups to keep here; older ones are removed after a good new one
#   BACKUP_AGE_RECIPIENT="age1..."  encrypt for these age public keys (space-separated). The archive holds the root key and the
#   BACKUP_AGE_RECIPIENTS_FILE=...  API keys in plain text, so encrypt it if it leaves this machine, or is kept anywhere others
#                                   can read. Only the matching private key (kept elsewhere) can open it, not this machine.
#   BACKUP_DEST=user@host:/dir/     also copy each backup there with rsync, over ssh if it has a host in it (a mounted folder
#   BACKUP_SSH_KEY=/root/.ssh/key   works too). The copy is added to and never pruned: the destination decides how long to keep.
#   BACKUP_SSH_OPTIONS="..."        ssh options; the default never asks a question and trusts a host the first time it sees it
set -euo pipefail
umask 0077

# Where things are. Not normally set: they are here so the tests can use a folder of their own.
STATE_DIR="${BACKUP_STATE_DIR:-/var/lib/home-display}"
CONFIG_FILE="${BACKUP_CONFIG_FILE:-/etc/home-display/config.toml}"
ENV_FILE="${BACKUP_ENV_FILE:-/etc/home-display/backup.env}"
BACKUP_DIR="${BACKUP_DIR:-/var/backups/home-display}"

# shellcheck disable=SC1090
if [ -r "$ENV_FILE" ]; then set -a; . "$ENV_FILE"; set +a; fi
BACKUP_DIR="${BACKUP_DIR:-/var/backups/home-display}"
BACKUP_KEEP="${BACKUP_KEEP:-14}"
BACKUP_AGE_RECIPIENT="${BACKUP_AGE_RECIPIENT:-}"
BACKUP_AGE_RECIPIENTS_FILE="${BACKUP_AGE_RECIPIENTS_FILE:-}"
BACKUP_DEST="${BACKUP_DEST:-}"
BACKUP_SSH_KEY="${BACKUP_SSH_KEY:-}"
BACKUP_SSH_OPTIONS="${BACKUP_SSH_OPTIONS:--o BatchMode=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$BACKUP_DIR/known_hosts}"
# The two programs the options use. Not normally set: for one that is somewhere else, and for the tests.
AGE="${BACKUP_AGE_BIN:-age}"
RSYNC="${BACKUP_RSYNC_BIN:-rsync}"

log() { echo "home-display-backup: $*"; }
die() { echo "home-display-backup: $*" >&2; exit 1; }

case "$BACKUP_KEEP" in '' | *[!0-9]*) die "BACKUP_KEEP must be a number, not '$BACKUP_KEEP'" ;; esac
[ "$BACKUP_KEEP" -ge 1 ] || die "BACKUP_KEEP must be at least 1"
[ "$(id -u)" -eq 0 ] || die "run it as root: the files are private to the service's user"

pki="$STATE_DIR/pki"
[ -d "$pki" ] || die "$pki is not there: nothing to back up (is the server installed, and has it been started?)"

# Encryption, if asked for: checked before anything is made, so a mistake is not found in the night.
age_args=()
for recipient in $BACKUP_AGE_RECIPIENT; do age_args+=(-r "$recipient"); done
if [ -n "$BACKUP_AGE_RECIPIENTS_FILE" ]; then
    [ -r "$BACKUP_AGE_RECIPIENTS_FILE" ] || die "BACKUP_AGE_RECIPIENTS_FILE $BACKUP_AGE_RECIPIENTS_FILE cannot be read"
    age_args+=(-R "$BACKUP_AGE_RECIPIENTS_FILE")
fi
if [ "${#age_args[@]}" -gt 0 ]; then
    # Nothing is made without it: an unencrypted backup when an encrypted one was asked for is the one thing not to do quietly.
    command -v "$AGE" > /dev/null || die "encryption is asked for (BACKUP_AGE_*) and age is not installed, so no backup was made: apt install age, or dnf install age"
    suffix=".tar.gz.age"
else
    suffix=".tar.gz"
fi

install -d -m 0700 "$BACKUP_DIR"
# One at a time: a backup by hand while the timer's runs would otherwise write over each other's partial file.
exec 9> "$BACKUP_DIR/.lock"
flock -n 9 || { log "another backup is running"; exit 0; }

# A second one in the same second must not write over the first, so a number is added when the name is taken.
# uname -n and not hostname: a minimal Fedora has no hostname command.
host=$(uname -n)
base="home-display-$(date -u +%Y%m%dT%H%M%SZ)-${host%%.*}"
name="$base$suffix"
number=1
while [ -e "$BACKUP_DIR/$name" ]; do
    number=$((number + 1))
    name="$base-$number$suffix"
done
partial=$(mktemp "$BACKUP_DIR/.partial.XXXXXX")
trap 'rm -f "$partial"' EXIT

# Paths are kept relative to /, so that they are restored where they were. The admin socket is not a file to keep, and a
# half-written file (.tmp) is not one either: every file is written whole and put in place by a rename, so what is copied while
# the server runs is either the old or the new, never a mix. If only the plain gzip is made, the plaintext is never on disk.
sources=("${pki#/}")
[ -r "$CONFIG_FILE" ] && sources+=("${CONFIG_FILE#/}")
set +e
if [ "${#age_args[@]}" -gt 0 ]; then
    tar --create --file=- --directory=/ --exclude='*.sock' --exclude='*.tmp' --warning=no-file-changed "${sources[@]}" \
        | gzip --no-name | "$AGE" "${age_args[@]}" > "$partial"
    status=("${PIPESTATUS[@]}")
else
    tar --create --file=- --directory=/ --exclude='*.sock' --exclude='*.tmp' --warning=no-file-changed "${sources[@]}" \
        | gzip --no-name > "$partial"
    status=("${PIPESTATUS[@]}")
fi
set -e
# tar's 1 is "a file changed while it was read", which the rename-into-place writes make harmless; anything else is a failure.
if { [ "${status[0]}" -ne 0 ] && [ "${status[0]}" -ne 1 ]; } || [ "${status[1]}" -ne 0 ] \
    || { [ "${#age_args[@]}" -gt 0 ] && [ "${status[2]}" -ne 0 ]; }; then
    die "making the archive failed (tar ${status[0]}, gzip ${status[1]}${status[2]:+, age ${status[2]}})"
fi
[ -s "$partial" ] || die "the archive is empty"

# A plain archive is read back; an encrypted one cannot be without the private key, which is not here, so `age -d` is the
# way to try it (home-display-restore --list does).
if [ "${#age_args[@]}" -eq 0 ]; then
    gzip --test "$partial" || die "the archive does not read back"
    # grep -c and not -q: -q stops reading at the first match, and tar then dies of the broken pipe, which pipefail reports.
    tar --list --gzip --file="$partial" | grep -c "^${pki#/}/" > /dev/null || die "the archive has nothing of $pki in it"
fi

mv "$partial" "$BACKUP_DIR/$name"
trap - EXIT
(cd "$BACKUP_DIR" && sha256sum "$name" > "$name.sha256")
log "made $BACKUP_DIR/$name ($(du -h "$BACKUP_DIR/$name" | cut -f1)$([ "${#age_args[@]}" -gt 0 ] && echo ', encrypted'))"

# Only after a good new one: the newest BACKUP_KEEP of what is here, by time.
removed=0
while IFS= read -r old; do
    rm -f -- "$old" "$old.sha256"
    removed=$((removed + 1))
done < <(find "$BACKUP_DIR" -maxdepth 1 -type f \( -name 'home-display-*.tar.gz' -o -name 'home-display-*.tar.gz.age' \) \
    -printf '%T@ %p\n' | sort -rn | tail -n +"$((BACKUP_KEEP + 1))" | cut -d' ' -f2-)
[ "$removed" -eq 0 ] || log "removed $removed older backup(s), keeping $BACKUP_KEEP"

# Off this machine, last, so that a destination that is down does not stop the local one: that is made, and this says so.
if [ -n "$BACKUP_DEST" ]; then
    # Checked here and not before the backup is made: without rsync the copy cannot be made, and the one here still can.
    command -v "$RSYNC" > /dev/null || die "BACKUP_DEST is set and rsync is not installed, so it was not copied (the backup here is good: $BACKUP_DIR/$name): apt install rsync, or dnf install rsync"
    ssh_command="ssh $BACKUP_SSH_OPTIONS"
    [ -z "$BACKUP_SSH_KEY" ] || ssh_command="$ssh_command -i $BACKUP_SSH_KEY"
    # 9>&- : without the lock. rsync starts helpers, and one that outlives this script would keep the lock, and every backup after
    # it would say "another backup is running" and do nothing. --timeout: a copy that stalls ends, and does not hold the unit.
    if "$RSYNC" --archive --partial --timeout=600 -e "$ssh_command" "$BACKUP_DIR/$name" "$BACKUP_DIR/$name.sha256" "$BACKUP_DEST" 9>&-; then
        log "copied to $BACKUP_DEST"
    else
        die "the copy to $BACKUP_DEST failed; the backup here is good ($BACKUP_DIR/$name)"
    fi
fi
