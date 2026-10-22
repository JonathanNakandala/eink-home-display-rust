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
sudo systemctl enable --now home-display home-display-health.timer
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

Back up `/var/lib/home-display`, above all `pki/root.key` and `pki/root.pem`: without them every display has to be paired again.
What is in it, and what to do if a part is lost: `deploy/README.md`, "What is kept, and what to back up". Stopping the service
first gives a copy that cannot be half written; a copy taken while it runs is safe for the files, which are written whole.

## Remove

`sudo ./install.sh --uninstall` removes the program and the units and keeps the configuration and the data.

## Licence

The server is **MIT OR Apache-2.0**, as you choose (`LICENSE-MIT`, `LICENSE-APACHE`). It contains other people's software, whose licences
ask that their notices go with it: `THIRD_PARTY_LICENSES.txt`, in the archive and in `/usr/share/doc/home-display-server/` for a package.
The weather icons are "Icons made by Prosymbols from www.flaticon.com", used under Flaticon's free licence, which asks for that credit.
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
