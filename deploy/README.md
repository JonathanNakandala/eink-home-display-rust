
# Monitoring the service

See [eink-home-display.service](eink-home-display.service) for running it under systemd.

The server answers two questions, on the same port as the image (`server.bind`):

- `GET /healthz`: `200` when the service is working, `503` when it is not. The body is one line:
  `ok`, `starting`, `degraded`, `failing` or `stale: the image is 3 h 0 min old`. Only **stale** is a
  failure: a scheduled render more than `server.stale_grace_seconds` (300) late, or still no image
  that long after start-up. A source being down is shown, but isn't a failed check, because the
  dashboard is still updating.
- `GET /status`: JSON with the state, the image's age and version, the last success and last failure
  (with the error text, cut to one line), the number of failures in a row, how each data source was
  on the last render, the next scheduled render, and the uptime.

```
curl --fail --silent http://localhost:8080/healthz          # exits non-zero when stale
curl --silent http://localhost:8080/status | jq '.state, .sources, .last_failure'
```

| State | Meaning |
|---|---|
| `starting` | No image yet, and the service has only just started |
| `ok` | The image is current and the last render was clean |
| `degraded` | Rendering, but a source is shown from old data or left off (see `sources`) |
| `failing` | The last render failed, but the image isn't old enough to be stale yet |
| `stale` | A scheduled render is long overdue. `/healthz` returns 503 |

For Docker, `HEALTHCHECK CMD curl --fail --silent http://localhost:8080/healthz`. For a plain systemd
unit, a timer that runs the same `curl --fail` and restarts the service on failure is enough; I haven't
tried it, and the unit file here doesn't include one. The endpoints have no authentication, like the
image, and the error text can name the failing source, so keep the port on the LAN.

`/status` keeps its history in memory, so a restart starts it again at `starting`. The sources list is
from the last render that succeeded.

## IPv4 and IPv6

`server.bind` defaults to `[::]:8080`, one socket that takes IPv4 and IPv6 clients. The server sets that up
explicitly (whether `[::]` also accepts IPv4 is otherwise a system setting) and announces over mDNS
exactly the IP versions it accepts: both address records for `[::]`, only the A record for `0.0.0.0`,
only the AAAA records for a specific IPv6 address. The log line at start-up says which:
`Serving the display image at http://[::]:8080/image (IPv4 and IPv6)`.

- **No IPv6 on the host** (disabled in the kernel, or a container without IPv6): the server falls back to
  `0.0.0.0` and says so (`listening on IPv4 only`), and announces only IPv4. A port that is already taken is
  an error, not a fallback.
- **Firewall:** allow TCP 8080 and UDP 5353 (mDNS) for IPv6 as well as IPv4. `ufw` does both when its
  `IPV6=yes`; with `nftables` use the `inet` table, with `ip6tables` add the rules separately.
- **What is announced:** each address only on its own interface, never loopback, so a client is not
  pointed at an address it can't reach. IPv6 privacy (temporary) addresses are still announced when the host has
  them, and they rotate: prefer a stable address (a DHCPv6 reservation, or a ULA) for anything that remembers
  the server's address. Check with `dns-sd -G v4v6 <instance-name>.local` (macOS) or
  `avahi-resolve -n <instance-name>.local` (Linux).
- Clients may log IPv4 peers as `::ffff:192.168.0.5`; that is the same address.

## Plain HTTP, HTTPS, or both

`[server] transport` chooses how displays reach the server. The default is `"http"`, which is how it has
always worked, so a configuration without it changes nothing.

| `transport` | Listens | Who is served | Protects against |
|---|---|---|---|
| `"http"` | plain HTTP on `bind` | everyone | nothing: anyone on the network can read the picture or pose as the server |
| `"prefer-https"` | plain HTTP on `bind`, and HTTPS on `[server.tls] bind` | everyone; over HTTPS a display that has joined is known by its certificate | someone listening, for displays that use HTTPS |
| `"https"` | HTTPS only | only displays that have joined (the way in is open so they can) | someone listening, and someone sending a display to plain HTTP |

`prefer-https` is for moving displays over one at a time. It does not stop someone who can interfere with
the network: they can make a display use plain HTTP, which is still there. Only `https` does, and only for
a display that is itself set to HTTPS-only in its firmware (see [esphome/README.md](../esphome/README.md)),
since that is the only display certain never to fall back.

```toml
[server]
transport = "prefer-https"      # "http" (default), "prefer-https" or "https"

[server.tls]
bind = "[::]:8443"
directory = "pki"               # the authority and the list of displays; back it up
names = ["eink.local"]          # what displays connect to; left out: <instance_name>.local and localhost
```

- **The authority.** The first start in either HTTPS mode makes a private certificate authority in
  `[server.tls] directory`: `authority.pem` (public: what displays are given to trust the server by) and
  `authority.key` (the secret, readable by its owner only). It lasts 20 years. **Back up the directory.**
  Without `authority.key` every display has to be paired again; the server will not quietly make a new one if
  only one of the two files is there. The server logs the authority's fingerprint at every start.
- **Which displays are members** is kept in `pairings.json` in the same directory. A file that can't be read
  stops the server from starting, instead of starting empty: forgetting the members would lock every display
  out, or let back in one that had been revoked.
- **Names.** A display checks the server's certificate against a name, so it must connect by a name in
  `names` (the `.local` name announced over mDNS is there by default). The certificate is made fresh in memory,
  lasts 90 days (`server_certificate_days`), and is replaced when a third of that is left, with no restart.
- **A display's certificate** lasts a year (`device_certificate_days`) and the display renews it itself while it
  is valid. Revoking a display takes effect on its next request, not when its certificate runs out; so does
  forgetting one and pairing another under the same name.
- **TLS 1.3 only.** Older versions are refused.
- **Announced over mDNS** beside the service, for a display to find: `tlsport` (where HTTPS is) and `secure`
  (`optional` when plain HTTP is served too, `required` when it is not). With `https` the service itself points at
  the HTTPS port.
- **Monitoring with `https`.** `/healthz`, `/status` and `/metrics` are display routes like the rest, so with
  `https` they need a member's certificate too. A monitor outside the network of displays can't reach them.

## Timezone

Set `timezone = "Europe/London"` (any IANA name, such as `America/New_York` or `Australia/Sydney`) under `[location]`.
The refresh schedule's hours and weekdays, the clock and date on the dashboard, and where a day starts are all in
that zone, whatever the machine's own zone is. A name that isn't an IANA zone stops the program at start-up and
says so.

Left out, the host's zone is used and a warning is logged at every start. That is only right if the machine is
set to the display's zone: a container or a cloud machine is usually UTC, and then a schedule written as
"07:00 to 09:00 on weekdays" runs at the wrong hours, with no error. The display takes its zone from the
server too: `/plan` carries the zone and its current offset from UTC, so the time in the display's "update
failed" notice is the server's, and there is nothing to keep in step in the firmware.

## Watching the displays

Each display names itself on every request (`device=` on `/plan`, `/refresh` and `/image`; the firmware
builds it from the chip's MAC address, so it is unique to the display and survives flashing). The server
keeps everything under that name and never infers who is asking from an address or from the order requests
arrived in, so several displays can't be mixed up; a request with no usable name is still served, just not
recorded. A display reports itself on its normal check-in (query parameters on `/plan` and `/refresh`, so no
extra radio time): its battery voltage, charge and state, how many wakes in a row failed, its Wi-Fi
signal strength (`wifi_rssi_dbm`), how long its previous wake was awake (`last_wake_seconds`, which is what
costs battery), and why its last failed wake failed (`last_failure`: `wifi`, `server`, `download`, `memory`
or `timeout`). A wake can't report its own failure, since it asks `/plan` before it knows how it will go,
so a failure is reported by the next wake that gets through. A Wi-Fi or server failure therefore shows up
once the display is talking to the server again, as "3 failed wakes in a row, the last because of wifi",
while `/plan` working but the image failing (`download`, `memory`, `timeout`) points at the server or the
display, not the radio. The server also knows when it told the display to come back, so it can tell one that is asleep from one that
has gone quiet: a display is **overdue** once it is later than that by `server.device_overdue_grace_seconds`
(15 minutes), which covers a slow Wi-Fi join but not a flat battery. An overnight sleep of seven hours is
not overdue, because the server asked for it.

- `GET /status` lists them under `devices`, with `age_seconds`, `overdue`, the battery, and `last_image`: the
  format and size of the last image that display was sent, and when.
- `GET /healthz` is still about the server only (it is 503 when the image is stale). A display going
  quiet is an alert on its own, below, not a reason to restart the service.
- `GET /metrics` is the same data in the Prometheus text format, for any scraper.
- The service logs a warning when a battery goes low or empty, a warning when a display reports a
  failure it hadn't before (and an info line when it is working again), and an info line when a display
  returns after being overdue.

### Scraping and alerting (Prometheus)

```yaml
scrape_configs:
  - job_name: eink
    scrape_interval: 1m
    static_configs:
      - targets: ["eink-host.local:8080"]
```

```yaml
groups:
  - name: eink
    rules:
      - alert: EinkDisplayOverdue
        expr: eink_device_overdue == 1
        for: 5m
        annotations:
          summary: "{{ $labels.device }} has not checked in when it was told to"
      - alert: EinkBatteryLow
        expr: eink_device_battery_state{state="low"} == 1
        for: 30m
        annotations:
          summary: "{{ $labels.device }} battery is low ({{ $value }})"
      - alert: EinkBatteryEmpty
        expr: eink_device_battery_state{state="empty"} == 1
        annotations:
          summary: "{{ $labels.device }} has stopped refreshing: battery empty"
      - alert: EinkDisplayFailingWakes
        expr: eink_device_failed_wakes >= 4
        annotations:
          summary: "{{ $labels.device }} has failed {{ $value }} wakes in a row (see eink_device_last_failure)"
      - alert: EinkDisplayWeakWifi
        expr: eink_device_wifi_rssi_dbm < -80
        for: 1h
        annotations:
          summary: "{{ $labels.device }} has a weak Wi-Fi signal ({{ $value }} dBm)"
      - alert: EinkServerStale
        expr: eink_render_state{state="stale"} == 1
        for: 5m
        annotations:
          summary: "The dashboard is not being re-rendered"
      - alert: EinkSourceUnavailable
        expr: eink_source_state{state="unavailable"} == 1
        for: 30m
        annotations:
          summary: "{{ $labels.source }} has had no data for half an hour"
```

A flat battery is the one failure the display cannot report itself, so `EinkDisplayOverdue` is the
alert that catches it: the display simply stops checking in. The metrics have no authentication, like
the rest of the server, so scrape from the LAN.
