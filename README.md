# Home display

Draws a dashboard (the weather, train departures) and serves it to e-ink displays on your network.

A server renders the page with headless Chrome on a schedule and publishes the picture. A display wakes, asks the server what is
current, downloads the picture only if it changed, draws it, and goes back to sleep for as long as the server says, which is what
makes a battery last for weeks. The server announces itself over mDNS, so a display finds it with nothing configured, and displays
join over HTTPS with a certificate from the server's own private authority, which you approve with one command.

```
 weather, departures ──► home-display-server ──(HTTP/HTTPS, found over mDNS)──► e-ink display
   Open-Meteo, OpenWeather,   renders with Chrome,                              Seeed reTerminal E1003
   National Rail, TfL         on a cron schedule                                (ESPHome firmware in esphome/)
```

**Status:** a personal project, provided **as is**: no support, no promise to answer issues or take contributions, and its interfaces may
change between releases. It runs here, and it is published so that it can be read and used, under the licences below, which come without
warranty.

## What is in it

| | |
|---|---|
| **The server** (Rust, `src/`) | Fetches the data, draws the layouts with Chrome, serves the pictures and a page for a browser, runs the private certificate authority, and is run with `displayctl`. Deployed on a Linux machine under systemd. |
| **The firmware** (`esphome/`) | For the Seeed reTerminal E1003 (ESP32-S3, 10.3 inch, 16 greys), built with ESPHome. Finds the server, checks it is still there at every wake, and joins it over HTTPS. |
| **A panel driven directly** | A Waveshare 7.5 inch V2 on a Raspberry Pi can be driven by the server itself (`display.kind = "WaveshareEpd7in5V2"`). |
| **Deployment** (`deploy/`) | An installer, `.deb` and `.rpm` packaging, the systemd units, daily backups with optional encryption and an off-machine copy, and the guides. |

## Install the server

On Ubuntu, Debian, Fedora and the like (x86-64 with AVX2), from a [release](../../releases): a `.deb` or `.rpm`, or an archive with
`install.sh`. Then edit `/etc/home-display/config.toml` and enable the service. **[deploy/INSTALL.md](deploy/INSTALL.md)** is the
guide, from installing to pairing a display, backing up and restoring; [deploy/README.md](deploy/README.md) explains what the server
does (layouts, HTTPS, monitoring, the API keys it uses).

Releases are immutable and signed by GitHub: check a download with `gh release verify`, and how it was built with
`gh attestation verify` (INSTALL.md, "Check what you downloaded").

## The display

[esphome/README.md](esphome/README.md) is the firmware's guide, and [esphome/FIRST_FLASH.md](esphome/FIRST_FLASH.md) is the checklist
for the first run on a real display. The firmware has your Wi-Fi password compiled in, so a release is its source and you build it
yourself: `make -C esphome compile`.

## Working on it

```sh
git config core.hooksPath .githooks     # fmt and clippy on commit, the tests on push
cargo test && cargo clippy --all-targets -- -D warnings
make -C esphome lint check test         # the firmware's checks (needs Poetry: `poetry install`, and clang)
```

Commits follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, ...): release-please makes the release
pull requests and the changelogs from them, one for the server and one for the firmware, and a merged release is made as a draft
and published by `.github/workflows/release-please.yml` once its checks, archive and packages have passed
([how, and what to do when it goes wrong](.github/release-please/README.md)). CI runs the tests, the licence and advisory checks, and
installs the packages on Ubuntu and Fedora.

## Licence

The server is **MIT OR Apache-2.0** ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). The firmware in `esphome/` is
**GPL-3.0-only** ([esphome/LICENSE](esphome/LICENSE)), because it is built with ESPHome. They are separate programs that talk over
HTTP. What the server contains of other people's is in `THIRD_PARTY_LICENSES.txt` with every release, and who wrote and licensed
each file is in [REUSE.toml](REUSE.toml) (checked in CI). Weather icons: [Meteocons](https://github.com/basmilius/meteocons) (MIT).
