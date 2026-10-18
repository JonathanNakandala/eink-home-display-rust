# First flash: what to check on a real display

Everything in the secure transport has been run against the real server on a computer, with the same mbedTLS the chip uses. None
of it has run on a chip. This is the order to find out, what to expect at each step, and what to write down. The numbers matter
as much as the pass or fail: the host tests cannot say how much heap a handshake needs on the chip or how long it takes there.

A box is ticked when you have seen the thing, not when you expect it.

## What you need

- The display, a USB cable (its serial bridge is on UART0, `board.yaml`), and the server running with a rendered image.
- The log: `poetry run esphome logs reterminal-e1003.yaml` over USB. The display is asleep most of the time; the log shows each
  wake as it happens. To keep it awake for a while, hold **KEY1** at boot (maintenance mode: the log says
  `Maintenance mode: staying awake`).
- To make it wake now: a **short press of KEY0** wakes it and asks the server for a fresh render (`Woken by the button`).
- On the server's machine: `displayctl`, and `curl` with `jq`:

  ```sh
  # What the server knows of each display (plain HTTP is served in prefer-https mode; with `https` it needs a member's certificate)
  curl -s http://SERVER:8080/status | jq '.devices[] | {name, firmware, battery_millivolts, wifi_rssi_dbm, failed_wakes, last_failure,
      last_wake_seconds, last_tls_milliseconds, last_heap_min_bytes, connection}'
  curl -s http://SERVER:8080/metrics | grep '^home_display_device'
  displayctl -c config.toml displays list
  ```

- A multimeter on the battery, once.

## The numbers to write down

Fill this in as you go. None has a known good value yet; the first run is what makes one.

| What | Where it comes from | Value |
|---|---|---|
| Heap left at the lowest, on the **join** wake | `last_heap_min_bytes`, on the wake after | |
| Heap left at the lowest, on an **ordinary** wake | `last_heap_min_bytes` | |
| First **full** handshake, as the display timed it | `last_tls_milliseconds` after the join | |
| First **full** handshake, as the server saw it | `connection.last_handshake_milliseconds` | |
| A **resumed** handshake, display and server | the same two, on a later wake | |
| Awake time of a wake that **only asks the plan** (picture unchanged) | `last_wake_seconds` | |
| Awake time of a wake that **downloads and draws** | `last_wake_seconds` | |
| Awake time of a wake that gets a **304** | `last_wake_seconds` | |
| Awake time of the **join** wake | `last_wake_seconds` | |
| The same wakes over plain HTTP (stage 1) | `last_wake_seconds` | |
| Wi-Fi signal where it will hang | `wifi_rssi_dbm` | |
| Battery voltage, log and multimeter | the log, multimeter | |
| Flash and RAM the build uses | the end of `esphome compile` | |

`last_wake_seconds`, `last_tls_milliseconds` and `last_heap_min_bytes` describe the wake **before** the one that reported them, so
a figure shows up one wake late.

A rule of thumb for the heap, not a measurement: a TLS 1.3 handshake with a client certificate wants tens of kilobytes. If the
lowest free heap is down in the low tens of KB, treat it as marginal, and watch for `last_failure: memory`.

## Before you flash

- [ ] `make -C esphome lint check test host-test` pass, and `make -C esphome compile` builds (a clean checkout of the tag you are flashing).
- [ ] `secrets.yaml` has the Wi-Fi name and password and an OTA password. `server_root` can be left empty (the root is fetched
  and confirmed by the pairing code); fill it to try the other way, stage 9.
- [ ] The server is running, has rendered an image, and `curl http://SERVER:8080/plan` answers.

## Stage 1: plain HTTP, as a baseline

Set `server_transport: http` and flash over USB. Nothing here is the secure transport, and it is the comparison for everything after.

- [ ] The boot log says `Project home-display.firmware-esphome version X.Y.Z`, the version you built.
- [ ] No `Not enough PSRAM` line. If there is one, its figures say how much was free: check PSRAM is detected (about 8 MB) before anything else.
- [ ] `3.9x V, NN%, ok` at the start of the wake, and `/status` lists the device with the same voltage. Compare with the multimeter once.
- [ ] `Found '...' at ... (http 8080, https 0 or N, format ...)`.
- [ ] `Image changed, downloading`, then `Image downloaded, drawing`; the picture appears, **not mirrored** (`mirror_x` is copied from
  Seeed's example), and the **greys look right** with `dithering: false`. A 1872x1404 grayscale image fitted in memory beside the framebuffer.
- [ ] The next wake logs `Image unchanged, skipping refresh`, and the sleep matches `next_seconds`; a wake that downloaded logs a sleep
  shorter than `next_seconds` by about the time it was awake.
- [ ] `shown_version` survives deep sleep: the second wake logs `Image unchanged, skipping refresh`, which the plan only says when `have=` was the version already on the panel.
- [ ] The remembered address survives deep sleep. Move the server to another port to see it recover (`No image server found`, then `Found`).
- [ ] `/status` shows the device with `firmware`, `battery_millivolts`, `wifi_rssi_dbm`, and after the second wake `last_wake_seconds`.
- [ ] Write down the awake times.

Battery and labels, without draining it: temporarily raise `battery_low_v` and `battery_empty_v` above the current voltage.

- [ ] The "Battery low" label appears, once; raising `battery_empty_v` makes it say `Battery empty - charge to resume` and stop.

Maintenance mode:

- [ ] Hold KEY1 at boot, let go: `Maintenance mode: staying awake`, and it is still up a minute later and reachable for an update over the air.

## Stage 2: failure and recovery, still plain HTTP

- [ ] Stop the server and wake the display a few times: a label `Last update failed @ HH:MM` appears **once** in the bottom-right corner,
  the right way round, with nothing else refreshed. The log says `Failure 1 in a row (server); sleeping ...`, and the sleeps run 10, 20, 40, then 60 minutes.
- [ ] Start the server: the next wake redraws the picture and the label goes. `/status` then shows `last_failure: server` once, from the wake before.
- [ ] Make the server accept the connection and never answer (`nc -l 8080` in its place): after 150 s the log shows
  `The wake didn't finish in 150s` and `Failure 1 in a row (timeout)`.

## Stage 3: join over HTTPS

Server: `[server] transport = "prefer-https"` (so plain HTTP is still served for monitoring and as a way back). Display:
`server_transport: prefer-https`, flashed over USB or OTA.

- [ ] `Found '...' (http 8080, https N, ...)`: the HTTPS port is announced.
- [ ] No `The clock is not set` line. If it appears on the first wake it is SNTP not having answered yet (the display waits up to `clock_wait`, 60 s, for it, does nothing that
  needs a certificate, and tries again at the next wake; `last_failure: clock` is reported). It should not persist: check Wi-Fi and the NTP servers in `clock.yaml`.
- [ ] Open the window: `displayctl -c config.toml window open`.
- [ ] After the picture, the panel shows `Waiting for approval: XXXX-XXXX-XXXX` in the corner (prefer-https goes on over plain HTTP meanwhile).
- [ ] The server's log says `... asked to join (reTerminal E1003, 1872x1404, 16 greys, firmware X.Y.Z)`, and `displayctl displays list` shows
  the same line: the display **says what it is** before you type anything.
- [ ] **The code on the panel is what you type.** `displayctl -c config.toml approve NAME CODE`. (Five wrong codes in a row lock it for 15 minutes.)
- [ ] The next wake: the server logs `Issued NAME a certificate`, `displays list` says `member`, the panel shows no notice, and the log has no `Not secure:`.
- [ ] Write down, from `/status` after the wake **after** the join: `last_heap_min_bytes` and `last_tls_milliseconds`, and the join wake's `last_wake_seconds`.

If it does not get there, `last_failure` says why (the table in the README, "Which failure it was"): `clock` (no usable time), `approval` (still waiting),
`certificate` (a certificate was refused, the server's or the display's), `unrecognised` (the server does not know this key), `memory` (flash could not be written or read, or the picture's buffer did not fit in PSRAM),
`server` (not reachable). `Not secure: <reason>` in the log is the same word.

## Stage 4: it is really TLS, and it resumes

- [ ] The plan and the picture now come over HTTPS: `/metrics` has `home_display_tls_handshakes_total{kind="full"}` or `"resumed"` rising,
  and `home_display_device_tls_handshakes_total{device="NAME",...}` for this display.
- [ ] From the **second** wake after joining: `home_display_device_tls_last_resumed{device="NAME"} 1`, and `resumed` rises by **one or two per wake** (the plan, and the picture when there is one to download).
  The saved session is in RTC memory, so this also shows RTC memory surviving deep sleep.
- [ ] `home_display_device_tls_last_handshake_seconds` is smaller on a resumed connection than on the full one. Write both down.
- [ ] If it stays `0`: the session is not being saved or offered. Check the clock (a session's age comes from the wall clock, so with no clock nothing is kept), that the display
  is not power-cycling between wakes (a power loss clears RTC memory), and `last_failure`.

## Stage 5: the picture that has not changed (304)

A re-render with identical bytes has a new `version` but the same `ETag`. The display asks with `If-None-Match`, the server answers 304, and the panel is not refreshed.

- [ ] Get two renders with identical bytes. That is hard to arrange while the dashboard shows a clock; it needs a render with nothing changed in it. To check the
  server's half without the display: `curl -sI -H 'Accept: image/png' http://SERVER:8080/image | grep -i etag`, force a render, and again; the same value means a display would get a 304.
- [ ] Expect `Same picture, keeping the panel`: no `Image downloaded, drawing`, **no refresh of the panel**, and a short awake time. Write it down. (This is not the
  `Image unchanged, skipping refresh` of stage 1, where the plan itself says nothing changed and no picture is asked for at all.)
- [ ] After a notice has been on the panel (for example the failure label), the next wake **does** download and redraw: a 304 must not leave a notice on the panel.

## Stage 6: the button

- [ ] Hold **KEY0** right after it wakes. At **5 s** the panel says `Keep holding to reset pairing (5 s)`. Let go at 7 s: nothing is reset, and the picture comes back **in the same wake**.
- [ ] Hold to **10 s**: the panel says `Pairing reset`, the log says `Pairing erased`, and the display goes on as a new one: it fetches the root and shows a new code.
- [ ] At the server the name is held as a **replacement** (`displays list` shows the new key's profile beside the member's). The **old** code does not approve it; the new code does. Then it is a member again.
- [ ] A short press still asks for a fresh render, as before. If the hold does not register at all, the pin read (`esp/button.h`: KEY0 is also the deep-sleep wake pin) is the first suspect.

## Stage 7: a certificate renewing

Waiting two months is not a test. Set `device_certificate_days = 1` under `[server.tls]` on the server and restart it; approve a display (or reset and re-join) so it has a
one-day certificate. A display renews with a third of its life left.

- [ ] Within a day, with nobody at the server, `displays list` shows a **later** end for the certificate, and the profile's firmware is current. Nothing was asked of the owner.
- [ ] `displays list` does not say the renewal is overdue. Put `device_certificate_days` back to 90.

## Stage 8: how it fails and how it falls back

- [ ] **A server with no HTTPS** (`transport = "http"`, restart) in front of a `prefer-https` display: the first wake logs `Not secure: server` and goes on over plain HTTP;
  the wakes after log `The server has no HTTPS (remembered): plain HTTP, and not looking again yet`, with no wait for the clock. Turn HTTPS back on: within a day the display finds it.
- [ ] **A revoked display**: `displayctl revoke NAME`. The next wake is turned away at its very next request; the panel says `Not recognised - ask the owner to approve: CODE`.
  `forget` it and approve again.
- [ ] **A server that trickles**: the picture is given up on at 45 s, the plan at 50 s, and the failure is `download` or `server`, not `timeout`.
- [ ] **`server_transport: https`** (plain HTTP never): with the display not yet approved it shows the code and sleeps for the server's retry (5 minutes by default) **without counting a failure**
  (`Waiting for the owner to approve this display; asking again in N s`). When the server is not serving HTTPS at all, it fails like any other wake and backs off.

## Stage 9: the owner's root compiled in (optional)

- [ ] Put the root in `secrets.yaml` as `server_root` (the base64 of its DER), reflash, reset the pairing. The display **does not fetch** the root; it joins as before.
- [ ] Put a different root in and join: `Not secure: certificate`, and nothing is kept.

## Over several days

- [ ] Leave it on a normal schedule. Each day: `/status` for `failed_wakes` (zero), `last_failure` (none), the battery voltage (write down the drop per day),
  `connection.resumed_handshakes` rising, `home_display_device_overdue` 0.
- [ ] The awake times stay what they were on day one. If they creep up, the handshake or the Wi-Fi is slowing.

## What this does not check

- **Flash encryption is not on.** The display's key is in plain NVS, and the TLS session in RTC memory. A display in someone's hand can be read.
- A pairing code is only as good as the owner comparing it; nothing here tests the owner.
- Nothing here runs for months. RTC memory across a power cut, a flat battery and a certificate that ran out are worth trying once each when you have a spare afternoon.

When you have the numbers, put them in the README beside the claims they replace (the heap and the handshake time are the two the design is least sure of).
