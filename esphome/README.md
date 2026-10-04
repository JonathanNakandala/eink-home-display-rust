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
```

Run it with a schedule, since the server only runs in that mode:
`cargo run --release --bin eink-home-display-rust -- -c config/default.toml --cron "*/10 * * * *"`

Check it with `curl -o /dev/null -w '%{size_download}\n' http://<host>:8080/image`.

## Device side

1. `cp secrets.example.yaml secrets.yaml` and fill it in.
2. Set `image_url` (the host's LAN address, ideally a DHCP reservation) and `image_format`.
3. `esphome run reterminal-e1003.yaml` over USB the first time.
4. To update later over the air, hold the middle button while waking; it then stays awake.

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

- The image isn't mirrored (`mirror_x` is copied from Seeed's example).
- A 1872x1404 GRAYSCALE `online_image` fits in memory alongside the framebuffer.
- Greys look right with `dithering: false`.
