# Installing the server on Linux

For a machine that runs the server all the time, under systemd: Ubuntu, Debian, Fedora and the like. Tested for syntax and
by the test suite; **not yet run on a real machine**, so treat the first install as a test and read the output.

## What you need

- x86-64 with AVX2 (Intel Core from 2013, AMD from 2017): the release is built for it. On anything else, build with
  `cargo build --release` and give `install.sh --bin-dir target/release`.
- systemd, and a clock that is kept right (`timedatectl set-ntp true`): the schedule and the certificates depend on it.
- **Chrome**, which draws the pictures. Google Chrome from google.com/chrome (a `.deb` or `.rpm`), or on Fedora
  `dnf install chromium`. **Not Ubuntu's `chromium` package**: it is a snap, which a service cannot start. Without one the
  server downloads Chrome into its cache on the first render, which works but needs the network and several hundred MB.
- `curl`, for the health check.

`install.sh` checks all of this first (`preflight.sh`, which you can also run on its own) and says what to do about anything.

## Install with a package

Each release has a `.deb` (Ubuntu, Debian) and an `.rpm` (Fedora, RHEL and relatives) beside the archive. The package does what
`install.sh` does, through the package manager, so it is the one to use if you want `apt` or `dnf` to know about it:

```sh
gh release download home-display-server-vX.Y.Z -R JonathanNakandala/eink-home-display-rust -p '*.deb' -p '*.rpm' -p '*.sha256'
sha256sum --check home-display-server_*.deb.sha256        # or the .rpm's
sudo apt install ./home-display-server_*_amd64.deb        # Ubuntu, Debian
sudo dnf install ./home-display-server-*.x86_64.rpm       # Fedora
```

It makes the user, puts the program in `/usr/bin`, the configuration in `/etc/home-display/config.toml` (kept on an upgrade, with a
changed one never replaced) and the certificate authority in `/var/lib/home-display` (first install only). **It does not start
the service**, since the configuration wants editing first:

```sh
sudoedit /etc/home-display/config.toml
sudo systemctl enable --now home-display home-display-health.timer home-display-backup.timer
sudo home-display-preflight            # what on this machine would stop it: Chrome, the firewall, the clock
```

An upgrade is the same command from the newer package, and restarts a service that was running. Removing the package keeps the
configuration, the data and the user: the certificate authority cannot be made again. The rest of this guide applies to both.
The package is built for x86-64 with AVX2 and Ubuntu 24.04's glibc (2.39) or newer, and will not install on a machine that is
neither. Its licence field says "Proprietary" until the repository has a licence file.

## Install from the archive

Without a package manager, or on any other systemd distribution:

```sh
# From the release (a private repository: with the GitHub CLI, logged in)
gh release download home-display-server-vX.Y.Z -R JonathanNakandala/eink-home-display-rust -p '*.tar.gz' -p '*.sha256'
sha256sum --check home-display-server-*.tar.gz.sha256
tar xzf home-display-server-*-x86_64-v3-linux.tar.gz
cd home-display-server-*-x86_64-v3-linux
sudo ./install.sh
```

It puts things here, and changes nothing else:

| What | Where |
|---|---|
| the program, and `displayctl` | `/usr/local/bin/` |
| the configuration, which it never replaces | `/etc/home-display/config.toml` |
| everything the service writes: the certificate authority, the list of displays, the pictures, the cache | `/var/lib/home-display/` |
| the service, and the health check's timer | `/etc/systemd/system/home-display.service`, `home-display-health.*` |
| the user it runs as | `home-display` (no login, no shell) |

On a machine with nothing in `/var/lib/home-display` it also makes the certificate authority, once. It stops with the
program's own message if the configuration is not valid, before it starts anything.

## Configure

Edit `/etc/home-display/config.toml`, then `sudo systemctl restart home-display`. Check a change first, without restarting:

```sh
sudo -u home-display /usr/local/bin/home-display-server --config-file /etc/home-display/config.toml --check-config
```

At the least: `[location]` (latitude, longitude, timezone), your departure boards, and `[schedule]`. The template says what each
section is for; `config/example.toml` in the archive has every setting, and `deploy/README.md` explains the features.

API keys (OpenWeather, the rail data, TfL) are given while it runs, so they need not be in a file:

```sh
sudo -u home-display displayctl -c /etc/home-display/config.toml credential set --help
```

A key lives in memory and is gone at a restart unless it is given with `--remember`, which keeps it in plain text in
`/var/lib/home-display/pki/credentials.json` (private to the service's user). On a machine that restarts by itself, remember them.

## Let a display join

With `transport = "prefer-https"` (the template's) or `"https"`, a display is known by its certificate and the owner approves it:

```sh
sudo -u home-display displayctl -c /etc/home-display/config.toml window open
```

Then flash the display (the firmware, in `esphome/`, has your Wi-Fi password compiled in, so you build it yourself from the
source at the release's tag: `esphome/README.md`), and follow `deploy/README.md`, "Letting a display join".

## Is it working

```sh
systemctl status home-display
journalctl -u home-display -f           # the start-up log says what the schedule means, in words
curl http://localhost:8080/healthz      # ok, or what is wrong
curl http://localhost:8080/status       # JSON: the last render, each source, the next one
```

`home-display-health.timer` looks at `/healthz` every three minutes and restarts the service after three failures in a row: a
process that is running but stuck, which systemd cannot see. A service that exits is restarted by systemd (30 s, then waiting
longer each time up to 30 minutes). With `transport = "https"`, `/healthz` needs a display's certificate, so the check cannot
look: put `HEALTH_URL=` (empty) in `/etc/home-display/health.env` to turn it off, or use `prefer-https`.

## Firewall

The displays reach the server on the two ports and find it over mDNS. Nothing here changes the firewall for you.

```sh
# Fedora (firewalld)
sudo firewall-cmd --permanent --add-port=8080/tcp --add-port=8443/tcp --add-service=mdns && sudo firewall-cmd --reload
# Ubuntu (ufw), with your own network
sudo ufw allow from 192.168.0.0/16 to any port 8080,8443 proto tcp && sudo ufw allow 5353/udp
```

The pages have no password, as the picture has none: keep the ports on your own network.

## Upgrade

The same, from the newer release. It restarts the service on the new program and leaves the configuration and the data alone.
A new setting that the release adds is in `deploy/config.production.toml`, which `install.sh` points you to, to compare. To go back,
install the older archive.

## Back up

A backup a day is made for you, and kept for two weeks, by `home-display-backup.timer` (`install.sh` turns it on, and for a package it is one of
the commands it prints; a machine that was off at the time is backed up soon after it starts). It holds what cannot be made again: the certificate
authority (`pki/root.key` above all, since without it every display has to be paired again), the list of displays, the keys kept
with `--remember`, and the configuration. The pictures and the cache are made again by the next render, so they are left out.

```sh
sudo home-display-backup                          # one now, the same way the timer makes it
sudo ls -l /var/backups/home-display              # the .tar.gz (or .tar.gz.age) and its .sha256, readable by root alone
systemctl list-timers home-display-backup.timer   # when the next one is
journalctl -u home-display-backup                 # what the last ones did
```

**A backup on the same disk does not survive that disk.** Both of these are optional, and both are settings in
`/etc/home-display/backup.env` (root only; the file explains each):

- **Copy it to another machine** (`BACKUP_DEST`): each backup is copied there with `rsync`, over ssh with a key that has no
  password, or to a folder that is a mounted disk. A destination that is down makes the backup fail loudly (`systemctl --failed`),
  after the one here is made. Copies are added and never removed: the destination decides how long to keep them.
- **Encrypt it** (`BACKUP_AGE_RECIPIENT`): the archive holds the root key and any API keys in plain text. Make a key pair
  *somewhere else* with `age-keygen -o key.txt`, keep `key.txt` safe (a password manager is a good place), and put its public key
  (it starts `age1`) in `backup.env`. This machine can then make backups and cannot open them, so a stolen disk or a copy on
  someone else's server is not the authority. Needs the `age` package.

**Restore**, on this machine or a new one. On a new one, install first (the package or `install.sh`), so that the user the files
belong to exists, and do not start the service:

```sh
sudo systemctl stop home-display
sudo home-display-restore --list FILE             # what is in it, changing nothing (add --identity key.txt for an .age one)
sudo home-display-restore FILE                    # checks it, puts what is there aside as *.before-restore-TIME, restores it
sudo home-display-restore --identity key.txt FILE.age
sudo systemctl start home-display
```

It refuses an archive that does not match its `.sha256`, that holds anything but the authority's folder and the configuration, or
that has a path that climbs out of them, and it never deletes what was there. **Try a restore once, now**, into a folder, to know it
works before you need it: `sudo RESTORE_ROOT=/tmp/try home-display-restore FILE` (and delete `/tmp/try` afterwards: it holds the
keys).

**To be told when a backup fails**, add a drop-in that starts something on failure, for example one that sends a message:

```sh
sudo systemctl edit home-display-backup      # then add:  [Unit]  OnFailure=notify-me@%n.service   (your own unit)
```

Not backed up: the cache and the pictures (remade), and the logs (the journal's). What each file in `pki/` is, and what to do if
only part of it is lost: `deploy/README.md`, "What is kept, and what to back up".

## Remove

`sudo ./install.sh --uninstall` removes the program and the units and keeps the configuration and the data.

## Licence

The server is **MIT OR Apache-2.0**, as you choose (`LICENSE-MIT`, `LICENSE-APACHE`). It contains other people's software, whose licences
ask that their notices go with it: `THIRD_PARTY_LICENSES.txt`, in the archive and in `/usr/share/doc/home-display-server/` for a package.
The weather icons are [Meteocons](https://github.com/basmilius/meteocons) by Bas Milius (MIT).
The display's firmware (`esphome/` in the source) is GPL-3.0-only, because it is built with ESPHome; it is a separate program.

## When it does not work

| What you see | What to look at |
|---|---|
| `install.sh` stops at "the configuration is not valid" | the message above it says which setting; fix `/etc/home-display/config.toml` and run it again |
| The journal says `Read-only file system` | a folder in the configuration is outside `/var/lib/home-display`: add it to `ReadWritePaths` in `systemctl edit home-display` |
| Chrome fails to start, and the log mentions the sandbox or a namespace | `preflight.sh` checks the usual causes. On Ubuntu 23.10 and later, `kernel.apparmor_restrict_unprivileged_userns`; on any system, `kernel.unprivileged_userns_clone` and `user.max_user_namespaces` |
| The same, on Fedora, and `getenforce` says Enforcing | SELinux may be refusing Chrome: `sudo ausearch -m avc -ts recent` shows what. Not tried; if that is it, a local policy module (`audit2allow -M`) for what it names is the fix |
| `status=203/EXEC` | the program is not where the unit says, or SELinux does not treat it as a program: `ls -Z /usr/local/bin/home-display-server`, then `restorecon -v` it |
| The picture is old and `/healthz` says `stale` | `journalctl -u home-display` for the render's error; each source's state is in `/status` |
| The displays do not find the server | the firewall above, and that mDNS gets through your network (`avahi-browse -rt _http._tcp`) |
