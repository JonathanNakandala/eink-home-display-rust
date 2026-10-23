#!/usr/bin/env bash
# Tries deploy/backup.sh and deploy/restore.sh on a made-up copy of the server's state, in a folder of their own, so nothing real is
# touched: a plain backup, what is left out, how many are kept, a restore, a damaged archive, an encrypted one, a copy to another
# place, and the things they must refuse. Run as root (the scripts refuse otherwise) from the repository root; needs age and rsync.
set -euo pipefail

[ "$(id -u)" -eq 0 ] || { echo "run as root: sudo $0" >&2; exit 1; }
command -v age > /dev/null && command -v age-keygen > /dev/null && command -v rsync > /dev/null || { echo "needs age and rsync" >&2; exit 1; }

repo=$(pwd)
backup="$repo/deploy/backup.sh"
restore="$repo/deploy/restore.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

pass=0
check() { # check "what" command...
    local what=$1
    shift
    if "$@" > /dev/null 2>&1; then pass=$((pass + 1)); echo "  ok    $what"; else echo "  FAIL  $what" >&2; failed=1; fi
}
fails() { ! "$@" > /dev/null 2>&1; }
failed=0

# --- a state like the server's: the authority and its friends in pki/, things that are not kept beside them
state="$work/var/lib/home-display"
config="$work/etc/home-display/config.toml"
mkdir -p "$state/pki" "$state/cache" "$state/served" "$(dirname "$config")"
for file in root.key root.pem intermediate.pem retired.pem pairings.json pairings.json.bak credentials.json viewers.json; do
    printf 'contents of %s\n' "$file" > "$state/pki/$file"
done
chmod 600 "$state/pki/root.key"
python3 -c "import socket,sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])" "$state/pki/admin.sock"
echo half > "$state/pki/pairings.json.tmp"
echo junk > "$state/cache/chrome"
echo picture > "$state/served/image.bmp"
echo 'transport = "https"' > "$config"

export BACKUP_STATE_DIR="$state" BACKUP_CONFIG_FILE="$config" BACKUP_DIR="$work/backups" BACKUP_ENV_FILE=/nonexistent
unset BACKUP_KEEP BACKUP_AGE_RECIPIENT BACKUP_AGE_RECIPIENTS_FILE BACKUP_DEST BACKUP_SSH_KEY

echo "plain backup"
"$backup"
archive=$(find "$BACKUP_DIR" -maxdepth 1 -name 'home-display-*.tar.gz' | head -n 1)
check "an archive was made" test -s "$archive"
check "with a checksum beside it" test -s "$archive.sha256"
check "private to root" test "$(stat -c %a "$archive")" = 600
listing=$("$restore" --list "$archive")
check "it holds the root key" grep -q "pki/root.key$" <<< "$listing"
check "and the list of displays" grep -q "pki/pairings.json$" <<< "$listing"
check "and the configuration" grep -q "etc/home-display/config.toml$" <<< "$listing"
check "not the admin socket" fails grep -q "admin.sock" <<< "$listing"
check "not a half-written file" fails grep -q "\.tmp" <<< "$listing"
check "not the cache or the pictures" fails grep -q -E "cache|served" <<< "$listing"

echo "how many are kept"
for _ in 1 2 3 4; do
    sleep 1.1
    BACKUP_KEEP=2 "$backup" > /dev/null
done
check "only 2 are left" test "$(find "$BACKUP_DIR" -maxdepth 1 -name 'home-display-*.tar.gz' | wc -l)" -eq 2
check "and their checksums, no others" test "$(find "$BACKUP_DIR" -maxdepth 1 -name '*.sha256' | wc -l)" -eq 2
archive=$(find "$BACKUP_DIR" -maxdepth 1 -name 'home-display-*.tar.gz' | sort | tail -n 1)

echo "restore into another place"
root="$work/root"
RESTORE_ROOT="$root" "$restore" "$archive" > /dev/null
check "the root key is back, as it was" cmp "$state/pki/root.key" "$root$state/pki/root.key"
check "with its mode" test "$(stat -c %a "$root$state/pki/root.key")" = 600
check "the configuration is back" cmp "$config" "$root$config"
RESTORE_ROOT="$root" "$restore" "$archive" > /dev/null
check "a second restore keeps what was there beside it" test -n "$(find "$root$state" -maxdepth 1 -name 'pki.before-restore-*')"

echo "a damaged archive"
cp "$archive" "$work/damaged.tar.gz"
cp "$archive.sha256" "$work/damaged.tar.gz.sha256"
sed -i 's/home-display-.*\.tar\.gz/damaged.tar.gz/' "$work/damaged.tar.gz.sha256"
printf 'x' >> "$work/damaged.tar.gz"
check "is refused" fails env RESTORE_ROOT="$work/root2" "$restore" "$work/damaged.tar.gz"
check "and nothing was put down" test ! -e "$work/root2"

echo "what it must refuse"
mkdir "$work/evil" && echo bad > "$work/evil/passwd"
tar -czf "$work/evil.tar.gz" -C "$work/evil" passwd
check "an archive with something else in it" fails env RESTORE_ROOT="$work/root3" "$restore" "$work/evil.tar.gz"
tar -czf "$work/up.tar.gz" -C "$work/evil" --transform "s,^,${state#/}/pki/../../,"  passwd
check "a path that climbs out" fails env RESTORE_ROOT="$work/root3" "$restore" "$work/up.tar.gz"
check "a backup with no authority to back up" fails env BACKUP_STATE_DIR="$work/nothing" "$backup"
check "a nonsense BACKUP_KEEP" fails env BACKUP_KEEP=none "$backup"

echo "encrypted"
age-keygen -o "$work/key.txt" > /dev/null 2>&1
public=$(age-keygen -y "$work/key.txt")
age-keygen -o "$work/other.txt" > /dev/null 2>&1
BACKUP_AGE_RECIPIENT="$public" "$backup" > /dev/null
encrypted=$(find "$BACKUP_DIR" -maxdepth 1 -name '*.tar.gz.age' | head -n 1)
check "an .age archive was made" test -s "$encrypted"
check "it is age's, not a gzip" grep -q "age-encryption.org" <<< "$(head -c 40 "$encrypted")"
check "without the key it is refused" fails env RESTORE_ROOT="$work/root4" "$restore" "$encrypted"
check "with the wrong key it is refused" fails env RESTORE_ROOT="$work/root4" "$restore" --identity "$work/other.txt" "$encrypted"
RESTORE_ROOT="$work/root5" "$restore" --identity "$work/key.txt" "$encrypted" > /dev/null
check "with the right key it is restored" cmp "$state/pki/root.key" "$work/root5$state/pki/root.key"
echo "$public" > "$work/recipients.txt"
BACKUP_AGE_RECIPIENTS_FILE="$work/recipients.txt" "$backup" > /dev/null
check "a recipients file works too" test "$(find "$BACKUP_DIR" -maxdepth 1 -name '*.tar.gz.age' | wc -l)" -ge 2

echo "a copy to another place"
mkdir "$work/dest"
BACKUP_DEST="$work/dest/" "$backup" > /dev/null
check "the archive is there" test -n "$(find "$work/dest" -name 'home-display-*.tar.gz' | head -n 1)"
check "and its checksum" test -n "$(find "$work/dest" -name 'home-display-*.tar.gz.sha256' | head -n 1)"
check "a destination that fails is a failure" fails env BACKUP_DEST=/proc/nowhere/ "$backup"
check "but the backup here was made" test -n "$(find "$BACKUP_DIR" -maxdepth 1 -newer "$work/dest" -name 'home-display-*' | head -n 1)"

echo
if [ "$failed" -ne 0 ]; then echo "FAILED" >&2; exit 1; fi
echo "all $pass checks passed"
