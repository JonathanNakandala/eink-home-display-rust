
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

## Timezone

Set `timezone = "Europe/London"` (any IANA name, such as `America/New_York` or `Australia/Sydney`) under `[location]`.
The refresh schedule's hours and weekdays, the clock and date on the dashboard, and where a day starts are all in
that zone, whatever the machine's own zone is. A name that isn't an IANA zone stops the program at start-up and
says so.

Left out, the host's zone is used and a warning is logged at every start. That is only right if the machine is
set to the display's zone: a container or a cloud machine is usually UTC, and then a schedule written as
"07:00 to 09:00 on weekdays" runs at the wrong hours, with no error. The display's own clock for the "last
updated" notice has a separate `timezone` in `esphome/reterminal-e1003.yaml`, which is not yet taken from the
server, so set the two to the same zone.

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
