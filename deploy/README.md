
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
```

### Starting for the first time

`prefer-https` makes the authority itself on the first start; displays carry on over plain HTTP meanwhile.
**`https` makes nothing unless asked**: run it once with `--init-pki`, then without. A directory with no authority
in it is most often a volume that was not mounted, and a new authority would lock out every display that holds a
certificate from the real one, so it is an error and not a fresh start. The same goes for the list of displays
(`pairings.json`): with an authority already there, a missing list is a lost file and an error, not an empty list.

### What is kept, and what to back up

The authority has two levels, as a private authority should: a **root** that displays pin, and an
**intermediate** that signs everything else. The root's key is used for one thing, signing a new
intermediate, so losing or leaking the intermediate never means pairing every display again.

| file in `[server.tls] directory` | what it is | secret? |
|---|---|---|
| `root.pem` | the root certificate (20 years): what displays are given to trust the server by | no |
| `root.key` | the root's key. Used only to make an intermediate. The server never loads it to serve | **yes** |
| `intermediate.pem` | the intermediate's certificate (5 years) and key, in one file so they are replaced together | **yes** |
| `retired.pem` | earlier intermediates, kept until they end, because what they signed is still good | no |
| `pairings.json` | which displays are members, written whole through a temporary file and a rename, with the version before the last change kept as `pairings.json.bak` | no |

- **Back up the directory**, `root.pem` and `root.key` above all: without them every display has to be paired
  again. The server logs the root's fingerprint at every start, so a changed one is visible.
- **`root.key` may live elsewhere.** It is on the server for now, so an intermediate that is near its end (under
  a year left) is replaced automatically at start-up. Moving it off the machine, to a password manager or a
  USB stick, is better: the server then runs as before, and warns at every start once the intermediate has
  under a year left. Put the key back, or give its path to the rotation, before the intermediate ends.
  If the root key is lost, the server keeps working until the intermediate ends; after that the displays must
  be paired again.
- **A lost or leaked intermediate** is replaced from the root without touching any display: delete
  `intermediate.pem` and start with `root.key` in place, and a new one is made. Displays keep their pin and
  their codes, and renew onto the new intermediate. (A leaked intermediate's certificates stay good until they
  end, but only members are served, so revoke what you don't trust.)
- **Which displays are members**: a file that can't be read stops the server from starting instead of starting
  empty, and says where the `.bak` is. If the file is deleted and the `.bak` is there, copy the `.bak` back:
  that loses only the last change, and a display whose record was lost with it asks again with the key it holds
  and is a member again, with no one at the server.
- **Certificates are not kept** except the display's own. The server's certificate is made fresh in memory.
- **An authority from before the two levels** (`authority.pem` and `authority.key`) is not read; remove them and
  pair again.

### Names

The server's certificate always has the name `eink-home-display.internal`, and that is the one the display
checks. Nothing in the configuration can change it, so renaming the instance or editing `names` never makes a
display stop trusting the server. `[server.tls] names` adds more (a browser or `curl` connecting by the `.local`
name, or by an address); left empty it is `<instance_name>.local` and `localhost`.

### How long things last, and what a display that is off does

- **A display's certificate lasts 90 days** (`device_certificate_days`) and the display renews it with a third
  of its life left. Short on purpose: renewal then happens all the time, so a display that has stopped renewing
  shows up within weeks and not after years of nobody remembering how it works. It is not what keeps a revoked
  display out; that takes effect on its next request.
- **A display that is off for longer than that**, or whose battery went flat, needs no one when it is next
  switched on. It sets its clock, sees its certificate has run out, and asks for a new one with the key it
  already holds. The server hands one out because it is the same key; the owner is involved only for a key
  the server hasn't seen. Nothing on the server ever removes a display for not being seen.
- **Changing a display's key** is possible and safe to do while it is off or asleep: the server keeps both the old
  and the new key as members until the new one is first used, however long that takes. It is not needed for
  renewal, and the firmware should leave it alone unless it has a reason.
- **A display that lost its key** (erased, or reflashed from scratch) asks to join under its old name. The
  member it was keeps working meanwhile, if it still can, and the new key waits for the owner to approve the
  code on its panel. Only then does it take the name over.
- **The server's own certificate** lasts 90 days (`server_certificate_days`) and is replaced when a third of
  that is left, with no restart. Displays trust the authority, not this certificate.
- **The server's clock.** It refuses to issue certificates while its clock reads before 2026, which means it was
  never set. Make sure the time service has set the clock before the server starts (the unit waits for
  `time-sync.target`).

### Seeing trouble before it is one

`/status` lists every display's standing under `members`: its state (`pending`, `approved`, `member`, `rejected`,
`revoked`), when its latest certificate ends and how long is left, whether that has run out (`certificate_expired`,
which fixes itself when the display is next on), and whether a key change or a replacement is waiting. `/metrics`
has the same as `eink_member_*`. A useful alert is a member that has stopped renewing, which shows as a certificate
close to its end:

```yaml
- alert: DisplayNotRenewing
  expr: eink_member_certificate_expiry_timestamp_seconds - time() < 14 * 86400
  for: 1h
  annotations:
    summary: "{{ $labels.device }} has not renewed its certificate (ends in under 14 days)"
```

A display that is simply off will trigger this, which is also the right thing to know.

- **TLS 1.3 only.** Older versions are refused.
- **Announced over mDNS** for a display to find. With `prefer-https` it is the usual `_http._tcp` service with
  two more keys: `tlsport` (where HTTPS is) and `secure=optional`. With `https` it is announced as
  **`_https._tcp`** on the HTTPS port (`secure=required`), and not as `_http._tcp` at all, so a browser or any
  client that goes by the service type is never sent to speak plain HTTP to a TLS port. Scan with
  `dns-sd -B _https._tcp` (macOS) or `avahi-browse -rt _https._tcp` (Linux).
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
