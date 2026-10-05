# reTerminal E1003

ESPHome config for the Seeed reTerminal E1003. It wakes on a timer, downloads the
latest dashboard from this project's `[server]`, draws it, and goes back to sleep.

```
Rust app  --cron "*/10 * * * *"  ->  <server.directory>/image.bmp  ->  GET /image
reTerminal wakes every 10 min    ->  online_image downloads it     ->  IT8951 draws it
```

## Rust side (config.toml)

```toml
[display]
kind = "ReTerminalE1003"
dither = "FloydSteinberg"   # or "Ordered"; the device does no dithering of its own
image_format = "Bmp"        # or "Png"; must match image_format in the yaml

[server]
bind = "0.0.0.0:8080"
directory = "served"
advertise = true                       # announce over mDNS, so a scan finds it
instance_name = "E-ink home display"   # the name a scan shows; the host name is e-ink-home-display.local
```

Run it with a schedule, since the server only runs in that mode:
`cargo run --release --bin eink-home-display-rust -- -c config/default.toml --cron "*/10 * * * *"`

Check it with `curl -o /dev/null -w '%{size_download}\n' http://<host>:8080/image`.

## Finding the server

The server announces itself over mDNS / DNS-SD as an ordinary `_http._tcp` service, with the
subtype `_eink-display` and the TXT keys `path=/image`, `format` and `version`. Any scan sees it:

```
dns-sd -B _http._tcp,_eink-display local.          # macOS: just this server
dns-sd -L "E-ink home display" _http._tcp local.   # its host, port and TXT record
avahi-browse -rt _http._tcp                        # Linux
```

Phone apps such as Discovery or Bonjour Browser list it too. The line saying
`can be reached at <host>.local.:<port>` gives the `image_url` to use. On the LAN that is
`http://e-ink-home-display.local:8080/image`, which is the default `image_url`. The device
resolves it through lwIP (`enable_lwip_mdns_queries`, set in the yaml). This is untested on the
hardware: if the log shows a resolve error rather than a connection error, use the IP address. The announcement is withdrawn when the app stops. Two servers on
one network need different `instance_name`s (a clash is resolved by adding a number).

## Device side

1. `cp secrets.example.yaml secrets.yaml` and fill it in.
2. `esphome run reterminal-e1003.yaml` over USB the first time. There is no address to set: the
   device finds the server itself.
3. To update later over the air, hold the middle button while waking; it then stays awake.

### What the server tells the device (`/plan`)

`GET /plan` (optionally `?have=<version>`) answers with JSON, worked out from the server's refresh
schedule. The device asks it on every wake.

```json
{"version":3973074790,"changed":true,"stale":false,"pending":false,"next_seconds":65,"age_seconds":1}
```

- `version`: when the image was rendered, in seconds since 1970, so a newer render has a larger version. The
  device sends back the version it is showing as `have`, and `changed` says whether it differs, so an image
  it already shows needs no refresh.
- `next_seconds`: how long to sleep: until the next scheduled render plus `server.wake_delay_seconds` (30).
  With a cron such as `*/10 6-22 * * *` the device sleeps until morning.
- `pending`: a render is due or running, so `next_seconds` is just the wake delay. Ask again then.
- `stale`: a scheduled render came more than `server.stale_grace_seconds` (300) late. `next_seconds` then points at the next slot.
- `age_seconds`: time since the image was rendered.

### What the device does on each wake

1. Joins Wi-Fi and asks `/plan?have=<version it is showing>` (the version is remembered in flash).
2. `changed: false`: skips the download and the refresh. `changed: true`: downloads the image, draws it,
   and remembers its `version`.
3. `stale: true`: also writes `Out of date: rendered 3 h 20 min ago` in the bottom-right corner.
4. Sleeps for `next_seconds` (limited to between 1 minute and 1 day). `sleep_duration` is only the
   fallback when the server never answered.

If `/plan` fails (no connection, a bad status, unreadable JSON) it takes the lookup-and-retry path
below, then counts a failed wake (see "When the download fails"). A device that has never drawn anything has version 0,
so its first wake always downloads.

### The refresh button

Press KEY0 (the right green button) to get a fresh picture now, for example in the middle of the night
when the device would otherwise sleep until morning. The button wakes the device, which then calls
`POST /refresh?have=<version>` instead of `GET /plan`. The server renders at once, waits for it to
finish (up to 40 s), and answers in the same format as `/plan`, so the rest of the wake is unchanged:
it downloads and draws the new image, then sleeps until the next scheduled render.

- The server refuses a request when a render started in the last `server.refresh_cooldown_seconds`
  (30), and then just answers with the image it has. A held or repeated press can't cause a stream of
  API calls.
- A forced render doesn't change the schedule: the next scheduled slot still happens.
- The panel doesn't change until the render is done, a few seconds in the log timings I saw on a laptop
  (2.4 s), longer on a Raspberry Pi or a cold Chrome. There is no on-screen "refreshing" message.
- If the server can't be reached, the usual lookup, retry and failure notice apply.

### How the device finds the server

[eink_discovery.h](eink_discovery.h) queries mDNS for `_http._tcp` services and takes the first one
whose TXT record has `txtvers=1` and a `path`, with an IPv4 address and a port. The result is
remembered in flash, so a normal wake does no lookup:

1. Download from the remembered address (or `fallback_url` before there is one).
2. If that fails, look the server up once, remember what is found, and download again.
3. If that fails too, keep the picture on the panel and sleep until the next wake.

So a changed IP or port is picked up on the next wake with no reflash. The lookup blocks for
up to 2.5 seconds, only after a failure.

With several servers on one network, set `server_name` to the Rust app's `instance_name`.
`fallback_url` (a `.local` name by default) only matters until the first scan succeeds, or if one
finds nothing; it needs `enable_lwip_mdns_queries`, set in the yaml.

The image format can't be found out at run time (`online_image` fixes it at compile time). The
server announces its format, and the log reports an error if it differs from `image_format`.

### When the download fails

The old picture stays on the panel. What happens next depends on how many wakes in a row have failed
(the count is kept in flash and resets on any wake that reaches the server):

| Failed wakes in a row | Panel | Sleep |
|---|---|---|
| 1st | Label in the bottom-right corner: `Last update failed @ 14:32` | `sleep_duration` (10 min) |
| 2nd | unchanged | 20 min |
| 3rd | unchanged | 40 min |
| 4th and later | unchanged | `max_backoff_ms` (1 h) |

So a server that is down for a day costs one panel refresh and a handful of short wakes, not 144.
When the server comes back, the next wake redraws the picture even if it is the version the device
thinks it shows, so the label doesn't stay up. The `Out of date` notice is also drawn once per stale
spell, not on every wake.

After deep sleep the device keeps no copy of the picture, so the label is a partial refresh of
just that corner ([eink_notice.h](eink_notice.h)). The display driver marks the whole screen as
changed at start-up, so the header clears that mark first; it reaches a protected member of the
driver to do so. If a future ESPHome renames it, the build fails rather than blanking the panel.

The time comes from the clock, which survives deep sleep. After a power loss with no Wi-Fi the
clock is unset, and the label reads `Last update failed` with no time.

## BMP or PNG

`BMP` is the default: an 8-bit greyscale file is about 2.6 MB, and the firmware streams
it with no decoder. `PNG` is a small fraction of that for a flat dashboard, but the
device has to inflate it into RAM. Try BMP first; if the radio-on time shows in battery
life, switch both settings to PNG. Measure rather than assume.

## Timing

The device sleeps for the `next_seconds` the server reports: the next scheduled render plus
`server.wake_delay_seconds`, so it wakes just after each render. A cron with quiet hours, such as
`*/10 6-22 * * *`, keeps it asleep overnight.

## Verify on first flash

- Stop the server and wake the device a few times: the label appears once, and the log shows
  `Failure 2 in a row; sleeping 1200 s`, then 2400 s. Start the server again: the next wake redraws
  the picture and the label goes.

- The log shows `Image changed, downloading` on the first wake and `Image unchanged, skipping refresh`
  on a wake before the next render, and the sleep length matches `next_seconds`.
- `shown_version` survives deep sleep (a second wake sends `have=` with the previous version).
- The lookup finds the server (`Found '...' at ...` in the log) and the remembered address
  survives deep sleep. Try moving the server to another port to see it recover.
- Unplug the Rust server and wake the device: the picture stays and the label appears in the
  bottom-right, the right way round, with nothing else refreshed.
- The image isn't mirrored (`mirror_x` is copied from Seeed's example).
- A 1872x1404 GRAYSCALE `online_image` fits in memory alongside the framebuffer.
- Greys look right with `dithering: false`.
