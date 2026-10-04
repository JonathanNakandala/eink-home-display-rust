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

## BMP or PNG

`BMP` is the default: an 8-bit greyscale file is about 2.6 MB, and the firmware streams
it with no decoder. `PNG` is a small fraction of that for a flat dashboard, but the
device has to inflate it into RAM. Try BMP first; if the radio-on time shows in battery
life, switch both settings to PNG. Measure rather than assume.

## Timing

`sleep_duration` should equal the cron period. The device's wake time isn't aligned to
the cron, so the picture can be up to one period old. Fine for a departures board; if it
matters, render earlier in each period than the device usually wakes.

## Verify on first flash

- The lookup finds the server (`Found '...' at ...` in the log) and the remembered address
  survives deep sleep. Try moving the server to another port to see it recover.
- The image isn't mirrored (`mirror_x` is copied from Seeed's example).
- A 1872x1404 GRAYSCALE `online_image` fits in memory alongside the framebuffer.
- Greys look right with `dithering: false`.
