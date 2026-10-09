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
image_format = "Bmp"        # or "Png" or "Qoi": sent when the display has no preference; all are published

[server]
bind = "[::]:8080"                     # IPv4 and IPv6; "0.0.0.0:8080" is IPv4 only
directory = "served"
advertise = true                       # announce over mDNS, so a scan finds it
instance_name = "E-ink home display"   # the name a scan shows; the host name is e-ink-home-display.local
```

Run it with a schedule, since the server only runs in that mode:
`cargo run --release --bin eink-home-display-rust -- -c config/default.toml --cron "*/10 * * * *"`

Check it with `curl -o /dev/null -w '%{size_download}\n' http://<host>:8080/image`.

## Finding the server

The server announces itself over mDNS / DNS-SD as an ordinary `_http._tcp` service, with the
subtype `_eink-display` and the TXT keys `path=/image`, `format`, `formats` and `version`. Any scan sees it:

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
3. To update later over the air, hold the middle button (KEY1) while waking. It is read once, at boot,
   and then the device stays awake until it is reset, so you can let go.

### How the YAML is laid out

`reterminal-e1003.yaml` is the device: its name, the C++ headers it includes, and what runs at boot. The rest is split
by concern into ESPHome [packages](https://esphome.io/components/packages/) in `packages/`: `board`, `battery`,
`clock`, `server`, `display` and `wake`. Each file holds the settings it uses (as `substitutions:`, with their
defaults and what they mean) beside the code that uses them, so changing how the battery behaves means opening
`battery.yaml` and nothing else. To change a default for one device, set the same name under `substitutions:` in
`reterminal-e1003.yaml`, which wins over a package's. Ids (globals, scripts, sensors) are shared between packages,
so a package may refer to another's, and the files only make sense together.

### Which display is which

The device's name is `reterminal-e1003-` plus the last three bytes of its chip's MAC address, such as
`reterminal-e1003-a1b2c3`: `name_add_mac_suffix: true` in the YAML. The MAC address is burned into the chip, so
the name is unique to that display, survives flashing and erasing, and lets the same file go to several
displays without them sharing a name. It is also the device's network name, so over-the-air updates and
`esphome logs` use it (`esphome run reterminal-e1003.yaml` finds it by the base name).

The device sends the name as `device=` on **every** request, `/plan`, `/refresh` and `/image`. The server keeps
everything it knows about a display under that name (battery, signal, the last wake, and the format and size of the
last image it was sent), and never works out who is asking from an address or from which request came before, so
two displays asking in the same minute can't be mixed up. The name has to be at most 32 characters of letters,
digits, `-`, `_` and `.`; anything else is ignored and the request is served as usual, just not recorded.

Changing the name makes the server see a new display, and the old name will show as overdue until the server is
restarted (it keeps them in memory only).

### What the server tells the device (`/plan`)

`GET /plan` (optionally `?have=<version>`) answers with JSON, worked out from the server's refresh
schedule. The device asks it on every wake.

```json
{"version":3973074790,"changed":true,"stale":false,"pending":false,"next_seconds":65,"age_seconds":1,"timezone":"Europe/London","utc_offset_seconds":3600}
```

- `version`: when the image was rendered, in seconds since 1970, so a newer render has a larger version. The
  device sends back the version it is showing as `have`, and `changed` says whether it differs, so an image
  it already shows needs no refresh.
- `next_seconds`: how long to sleep: until the next scheduled render plus `server.wake_delay_seconds` (30).
  With a cron such as `*/10 6-22 * * *` the device sleeps until morning.
  Never more than a day: over a longer quiet spell (a weekend with no refreshes) the device wakes once a day,
  finds the picture unchanged and sleeps again, and the server still notices a display that has gone quiet.
- `pending`: a render is due or running, so `next_seconds` is just the wake delay. Ask again then.
- `timezone`, `utc_offset_seconds`: the zone the server works in (`[location] timezone`), and how far ahead of UTC it is
  right now. The device has no timezone database and nothing in its YAML names a zone: it keeps the offset from the last
  plan (across sleeps, since a failure is often a server it couldn't reach) and adds it to its clock to write the time
  in the failure notice (`Last update failed @ 14:32`). It is right until the next clock change, which the next plan
  corrects. Until a plan has ever answered, or if the clock isn't set, the notice has no time rather than a wrong one.
- `stale`: a scheduled render came more than `server.stale_grace_seconds` (300) late. `next_seconds` then points at the next slot.
- `age_seconds`: time since the image was rendered.

### What the device does on each wake

1. Joins Wi-Fi and asks `/plan?have=<version it is showing>` (the version is remembered in flash).
2. `changed: false`: skips the download and the refresh. `changed: true`: downloads the image, draws it,
   and, once the panel has finished refreshing, remembers its `version`. If the device resets mid-refresh the
   old version is still recorded, so the next wake downloads the image again.
3. `stale: true`: also writes `Out of date: rendered 3 h 20 min ago` in the bottom-right corner.
4. Sleeps for `next_seconds` (limited to between 1 minute and 1 day), less the time the wake has used
   since `/plan` answered, so it comes back when the server meant and not a download, a refresh and a
   settle later (never under a minute). `sleep_duration` is only the fallback when the server never answered.

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

[eink_discovery.h](eink_discovery.h) queries mDNS and takes the first service whose TXT record has `txtvers=1` and a
`path`, with an IPv4 address and a port. Which service type it browses depends on `server_transport` (a substitution in
[packages/server.yaml](packages/server.yaml)): `_https._tcp` for `https`, and `_http._tcp` for `http` and `prefer-https`,
as under "Finding it" below. What to make of an answer is decided in [eink_service.h](eink_service.h), which is tested on
a computer. Besides the HTTP port it remembers the `tlsport` the server announces, for the secure transport; nothing
uses it yet, so whatever `server_transport` says, the firmware still speaks plain HTTP. An unknown value stops the
build, so a typo can't quietly leave a display on plain HTTP. The result is remembered in flash, so a normal wake does
no lookup:

1. Download from the remembered address (or `fallback_url` before there is one).
2. If that fails, look the server up once, remember what is found, and download again.
3. If that fails too, keep the picture on the panel and sleep until the next wake.

So a changed IP or port is picked up on the next wake with no reflash. The lookup blocks for
up to 2.5 seconds, only after a failure.

With several servers on one network, set `server_name` to the Rust app's `instance_name`.
`fallback_url` (a `.local` name by default) only matters until the first scan succeeds, or if one
finds nothing; it needs `enable_lwip_mdns_queries`, set in the yaml.

### Secure transport (what the firmware has to do)

*Status: built, and in the wake script (`server_transport`, [packages/secure.yaml](packages/secure.yaml)), but **not yet run on a
device**. Everything that can be run on a computer has been, against the real server and the same mbedTLS the chip uses
([host/](host/README.md)): the pairing code, joining, renewal, the flash record and a power cut in the middle of writing it, the
TLS and EST client, the image streaming in megabytes, and the choice of route in each of the three transports. What has not been
tried is the chip itself: heap during a handshake, time and battery per wake, NVS, RTC memory through deep sleep, and the panel.
Treat `https` as untested hardware until it has been, and keep `http` (the default) until then.*

The server can offer HTTPS (see `[server] transport` in [deploy/README.md](../deploy/README.md)). The display
has the same three choices, as a substitution, and its choice matters as much as the server's: the server can
only offer, and only a display set to HTTPS-only is certain not to fall back.

| display `transport` | does | when HTTPS can't be used |
|---|---|---|
| `http` | plain HTTP only; ignores what the server offers | n/a |
| `prefer-https` | HTTPS if the server announces `tlsport` and the display has joined; otherwise HTTP | **falls back to HTTP on any failure**, and says so in the notice so the owner can see it. Someone who can break the TLS can also strip `tlsport` or block the port, so refusing to fall back would add no protection, only a display that stops working. This mode is for moving over, not for protection |
| `https` | HTTPS only, never HTTP, whatever is announced | the wake fails like any other failure; the picture stays |

**Finding it.** What the server announces depends on its `transport`:

| server `transport` | service type | port | extra TXT keys |
|---|---|---|---|
| `http` | `_http._tcp` | HTTP | none |
| `prefer-https` | `_http._tcp` | HTTP | `tlsport` (the HTTPS port, same address), `secure=optional` |
| `https` | **`_https._tcp`** | HTTPS | `tlsport` (the same port), `secure=required` |

A display in `http` or `prefer-https` mode browses `_http._tcp`, as now, and reads `tlsport` if it wants HTTPS. A
display in `https` mode browses **`_https._tcp`** only: that is the only place a TLS-only server appears, and it
must not look under `_http._tcp`, where nothing of this server's is listed in that mode. Both use the same
`_eink-display` subtype, `txtvers=1` and `path` as before, and the same instance name, so `server_name` works the
same way.

**TLS.** TLS 1.3 only. The display connects to the address mDNS gave it and verifies the certificate against
the fixed name **`eink-home-display.internal`**, which the server's certificate always has and no configuration can
remove, not against the address and not against the mDNS name (which the owner can change). It trusts exactly one
certificate authority, the server's root, which it gets in one of the two ways under "Which root the display trusts"
below. The server sends two certificates in the handshake (its own and the intermediate, about 480 bytes each),
ECDSA on P-256, and the display builds the path to the root it pins. The intermediate is replaced every few
years, and nothing on the display changes when it is. The name matters: a display's own certificate
has none, so it can't pass for the server to another display. (mbedTLS can check a name other than the address
connected to, with `mbedtls_ssl_set_hostname`: shown by [host/](host/README.md), which runs the display's side of this
against the real server on a computer. Heap and speed on the chip are still to be measured.)

**Which root the display trusts.** There are two ways, and only the first step of joining differs:

| | `server_root` left out (**default**) | `server_root` set in `secrets.yaml` |
|---|---|---|
| Where the root comes from | fetched from the server on first contact and confirmed by the pairing code | the owner's `root.pem`, compiled into the firmware |
| First contact | an unverified TLS connection to `GET cacerts` (step 1 below) | none: the display verifies the server from its first connection |
| Someone in the middle at pairing | caught by the pairing code | cannot happen |
| One firmware for every home | yes | no: each owner builds their own, as they already do for Wi-Fi |
| If the root ever changes | the display pairs again | reflash |

The default is the easier one to use: flash once, then approve the code on the panel. `server_root` is for an owner
who would rather not rely on the code for that first step, and costs a line in `secrets.yaml`. Everything after the
root is known is the same either way, including the pairing code on the panel (it still tells the owner which display
is asking). A display with `server_root` set that is shown a different root refuses it and says so; it never
falls back to fetching one.

**Joining (EST, RFC 7030 as updated by RFC 8951; see `reference/`).** All under `/.well-known/est/`, TLS 1.3:

1. First wake with no root stored (and none compiled in): connect to `tlsport` **without verifying the server** and `GET cacerts`.
   The body is base64 of a DER CMS `SignedData` (`certs-only`) holding two certificates: the **root** and the
   **intermediate** under it. Pick out the root, the one that is self-signed (its subject and issuer are the
   same and its signature checks against its own key), and keep only that, in memory for now. Do not keep the
   intermediate: the server sends it with its own certificate in every handshake, and completes a display's
   path itself, so the display never has to keep or send one. (It does need it once, to check the certificate it is
   given: that comes alone, and the intermediate is taken from the handshake it arrived on, see [host/](host/README.md).)
   (With `server_root` set this step is skipped.)
2. Make an ECDSA P-256 key (kept in flash, never leaves the display, not regenerated on renewal) and a PKCS #10
   request signed with ECDSA and SHA-256, with the display's name (the same name it sends as `device=`) as its
   only common name. Other fields are ignored. If the server's `csrattrs` includes the challenge-password OID, put
   in `challengePassword` the base64 of the connection's RFC 9266 channel binding: the TLS 1.3 exporter with
   label `EXPORTER-Channel-Binding`, no context, 32 bytes.
3. **Work out the pairing code and show it on the panel**, from what the display itself saw (never from anything
   the server says; the server does not send it):
   `SHA-256("eink-home-display pairing code v1" || SHA-256(authority DER) || u64be(len(name)) || name ||
   u64be(len(SPKI)) || SPKI)`, then the first 60 bits (the first 8 bytes as a big-endian number, shifted right by 4) as twelve 5-bit values, most
   significant first,
   written in Crockford's Base32 (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`, upper case) as `XXXX-XXXX-XXXX`. SPKI is the DER
   `SubjectPublicKeyInfo` of the display's key. Show it in a large font, upper case; the alphabet leaves out
   `I`, `L`, `O` and `U`, and the server reads a typed `I`/`L` as `1` and `O` as `0`, in any case.
   Example: authority DER `30 03 02 01 01`, name `reterminal-e1003-a1b2c3`, SPKI the 91 bytes `00 01 .. 5a`
   give `B0AJ-QTW6-Y8SA`. The authority DER here is the **root's**. The owner types this at the server; if someone is between the two, the codes differ and
   the approval fails. That comparison is the only thing that authenticates the first contact.
4. `POST simpleenroll` (`Content-Type: application/pkcs10`, body base64 of the DER request, no
   `Content-Transfer-Encoding`). `202` with `Retry-After`: not approved yet; sleep that long and ask again (a new
   request on a new connection, so a channel binding is made again). `200`: the body is base64 of a CMS `SignedData`
   with the display's certificate. Store it. `403` means the owner declined, or the window isn't open.
5. From then on verify the server against the stored authority, and show the certificate as the TLS client
   certificate. The server names the display by it, ignoring `device=`.
6. Renew with `POST simplereenroll` over a connection that shows the current certificate, with a third of its
   life left (the server issues 90 days, so at 30), with the same key, checking on every wake and retrying on the
   next if it fails. Write the new certificate to the spare of two slots and switch to it only once it is complete,
   so a power cut in the middle leaves the old one.
7. Changing the key is possible and safe (the server keeps both until the new one is first used), but there is
   no reason to, so leave it.

**The rule under all of this: pairing is removed only by the owner, never by an error.** The key, the certificate
and the authority are not deleted because a connection failed, a certificate was refused, a response was `403`, the
clock was wrong or the server was unreachable. Only a deliberate action (a long press, or a flash that erases)
clears them. They are stored as one versioned blob with a checksum, so a half-written one is recognised and the
previous one used.

**After being off for a while** (a flat battery, a drawer), in this order:

1. The clock is wrong after power was lost. **Set it by SNTP before any TLS**; certificate dates are checked against
   it. If it can't be set, that is "try again next wake", never "unpaired".
   The `ensure_clock` script does this: it waits up to `clock_wait` for SNTP, then sets `clock_ok`. The clock counts as
   usable when it reads 2026-01-01 or later (the same date the server refuses to make certificates before) and is
   not earlier than the newest render the display has been told of, since a clock behind something that already
   happened is wrong too ([eink_clock.h](eink_clock.h), tested on the host). The servers come from `ntp_server_1` to
   `ntp_server_3`, tried in that order, by default three public ones from different operators. A local one (your
   router, or a machine running chrony) can go first, but only if it is really there: a server that does not
   answer is given up on after a wait, which is radio time on every wake. Until the clock is usable, nothing that
   needs TLS is tried.
2. If the certificate's end is earlier than the clock now, it has expired: connect **without** showing it and
   `POST simpleenroll` with the same key. The server gives a new certificate to the key it already holds, with no
   owner and no window. Store it and carry on.
3. If the server answers `403` to that, the key is not one it knows (for instance after an erase). Show the code for
   the key and a notice that the owner has to approve it; keep asking, with the `Retry-After` given. If it was a member
   before, its old key keeps working on the server meanwhile, if it still has it.

**What to tell the owner, on the panel** (each different, none blank): waiting for approval and the code; the clock
is not set; the server can't be reached; the certificate is refused (and by what); not recognised, ask the owner to
approve. And report which in the telemetry's `last_failure`, so `/status` says why.

**How the firmware is organised.** The same split the other `eink_*.h` headers use: what can be decided without
hardware is pure and is tested on the host (`make -C esphome test`); what touches the radio, the flash or mbedTLS is
thin and does as little deciding as it can.

| Layer | File (proposed) | Does | Tested |
|---|---|---|---|
| Pure logic | [eink_pairing.h](eink_pairing.h) (**built**) | the pairing code (hash, Crockford encoding), and which step comes next from what is stored, the clock and the answer last received | on the host, against the pinned test vector `B0AJ-QTW6-Y8SA`, the same one the server pins |
| Pure logic | [eink_trust.h](eink_trust.h) (**built**) | which root to trust (compiled in, stored, or none yet), whether a certificate has expired or is due to renew | on the host |
| Pure logic | [eink_sha256.h](eink_sha256.h) (**built**) | SHA-256 for the code, the same on the chip and in the tests; checked against the standard's examples | on the host |
| Pure logic | [eink_join.h](eink_join.h) (**built**) | one wake's joining: decide ([eink_pairing.h](eink_pairing.h)), do it through the interfaces below, say where it got to. Keeps the root only once a certificate has been issued under it | on the host, against fakes |
| Interfaces | [eink_ports.h](eink_ports.h) (**built**) | what joining needs from outside: `Clock`, `Identity` (key, certificate, root, the last answer), `Est` (the network) and `Verifier` (a certificate chains to a root). The chip implements them; the tests use [fakes](tests/fakes.h) | n/a |
| Storage | [eink_credentials.h](eink_credentials.h), [eink_flash_identity.h](eink_flash_identity.h), [eink_nvs.h](eink_nvs.h) (**built**) | the key, the root, the certificate and its dates as one record with a version, a sequence number and a CRC-32, written whole to one of two slots in turn, so a write cut short leaves the record before it. `FlashIdentity` is the interface the joining logic uses over it; `NvsStore` is ESP-IDF's NVS under it (namespace `eink`, never erased from here). A record that cannot be read is left alone and nothing is written over it | the record, the slots and every way a write can be cut short, and `FlashIdentity` against the real server, with a flash that loses power: on the host. NVS itself: on the device |
| Transport | [eink_tls.h](eink_tls.h), [eink_est_client.h](eink_est_client.h), [eink_csr.h](eink_csr.h), [eink_verifier.h](eink_verifier.h) (**built**, with [eink_der.h](eink_der.h), [eink_base64.h](eink_base64.h), [eink_http.h](eink_http.h) and [eink_calendar.h](eink_calendar.h) under them) | a TLS 1.3 connection, EST over it (fetch the authority, ask, renew), the request, the check that a certificate chains to the root. They implement the interfaces above with mbedTLS and BSD sockets (which lwIP has too) | the pure parts on the host; all of it against a real server with the same mbedTLS (`make -C esphome host-test`) |
| Transport | [eink_stream.h](eink_stream.h), [eink_body.h](eink_body.h), [eink_secure.h](eink_secure.h) (**built**) | a request whose answer is read as it comes, so the image (megabytes) is never held: the head, then the body by length or in chunks, in whatever pieces the network gives it; `eink_secure.h` is where the wake script says once where the server is and what to show it | the framing on the host; all of it against a real server, with a 2.6 MB body read in pieces of 1.4, 4 and 16 KB |
| Transport | [eink_secure_http.h](eink_secure_http.h) (**built**) | a `HttpRequestComponent` and `HttpContainer` over that stream, so `online_image` downloads the picture through it unchanged: the script points it at the new component with `set_parent` and back at the stock one if it falls back | on the device (needs ESPHome's headers); `esphome compile` |
| Discovery | `eink_discovery.h` (extended) | `_https._tcp` or `_http._tcp`, `tlsport`, `secure` | on the device |
| Orchestration | [eink_secure_wake.h](eink_secure_wake.h), [eink_secure_begin.h](eink_secure_begin.h), [eink_secure_chip.h](eink_secure_chip.h), [packages/secure.yaml](packages/secure.yaml) (**built**) | the choice of route from the transport and how the join went; a wake's beginning (join, decide, record the server); the chip's clock, flash and watchdog under it; and the scripts that call them | the choices and the beginning on the host (the beginning against the real server, in every transport); the scripts and the chip's part: `esphome compile` only |

The stock `http_request` component cannot be used for the secure path: it takes its CA certificate at compile time,
sets no name to check, and has no client certificate. So `eink_secure_http.h` subclasses `HttpRequestComponent` and
overrides `perform`, and the script points `online_image` at it with `set_parent` (no external component or Python needed).
The `http_request` actions for the plan stay on the stock component and are used for plain HTTP only; over TLS the plan goes
through `eink_secure_chip::fetch`, which is a small request read whole.

### What a wake does with it (`server_transport`)

[packages/secure.yaml](packages/secure.yaml), after Wi-Fi: `begin_server` runs the plain `check_plan` for `http`, and
`secure_session` for the other two. That waits for the clock (`ensure_clock`), looks for the server if it does not know its HTTPS
port, joins ([eink_join.h](eink_join.h)) and takes the route [eink_secure_wake.h](eink_secure_wake.h) gives:

| After the join | `prefer-https` | `https` |
|---|---|---|
| paired | TLS | TLS |
| waiting for the owner, or not recognised | **plain HTTP**, and the pairing code is drawn in the corner once, until the display is paired | **no request**: the code is drawn once, and it sleeps for the server's `Retry-After` (five minutes if it said none) without counting a failed wake, so the owner is not left waiting on a backoff |
| clock not set, server not found or no HTTPS port, certificate refused, flash unreadable | **plain HTTP**; the reason is told to the server at the next wake that gets through | the wake fails like any other (backoff, notice with the reason) |
| a TLS request fails after joining | retried over plain HTTP, the stock component put back for the picture | the wake fails with `certificate` (a certificate refused) or `server` |
| the picture's download fails | tried once more over plain HTTP | the wake fails with `certificate` or `download` |

`https` never ends up on plain HTTP; a test pins that for every outcome. The pairing code is a notice in the corner like the
others, kept across sleeps so it is drawn once and not every wake (`prompt_on_panel`), and cleared when the picture or any other
notice is drawn over it.

**Resuming a session.** A TLS 1.3 handshake with a client certificate is the most that one wake asks of the radio and the
battery, and a wake makes two (the plan, the picture). The server gives the display a ticket on each connection, and
[eink_session_cache.h](eink_session_cache.h) keeps the newest, so that the next connection (the picture, and the plan ten minutes
later) resumes it: no certificate exchange, no signatures. Resumption is for the data connections only; the first contact and the
enrolment are made in full, since they are tied to their own connection (RFC 9266) and nothing about a display is kept before the
owner approves it.

- It is kept in **RTC memory** (3 KB of the 8 KB there; the session itself is about 700 bytes): it survives deep sleep, and a flat
  battery just means one full handshake. It is secret like the key, and is where the key is anyway for anyone holding the device.
- It is keyed on the server's address and port, the root and the display's certificate, so another server, a changed root and a
  **renewed certificate** each start afresh.
- mbedTLS dates a ticket by a clock that restarts at deep sleep, so the cache ages it by the wall clock instead; see
  [host/README.md](host/README.md). With no clock that can be believed nothing is kept or offered.
- Anything that stops a saved session working (the server was restarted with other keys, a session from another build, damage)
  is forgotten and the connection is made again in full, once, in the same request: never a failed wake.
- The server still checks membership on every request, so a revoked display is turned away on a resumed connection too.
- The server's `/metrics` counts handshakes by kind (`eink_tls_handshakes_total{kind="resumed"}`), which is how to see it working
  on a real display: it should climb by about two per wake after the first.

Limits worth knowing:

- A display set to `prefer-https` against a server that does not serve HTTPS looks for it on every wake (about 2.5 s of radio),
  since it never learns an HTTPS port. Set it to `http` for such a server.
- The flash grows by about 26 KB over what the TLS options cost, whatever `server_transport` is, as the code is part of the
  wake script even when it is not used.
- `server_root` (the owner's root in the firmware, in `secrets.yaml`) is read as the base64 of the DER, one line.

The states, so each has one meaning on the panel and in `last_failure`: *no root yet* (fetching it), *asking*
(has a key and a root, no certificate), *waiting for approval* (shows the code), *paired*, *renewing*, *expired*
(asking again with the same key), *not recognised* (the server doesn't know this key: the owner must approve), and
*clock not set* (nothing else can be tried until it is).

### Which image format: content negotiation

The server publishes every format it can send (BMP and PNG) on each render, and `GET /image` picks one
from the request's `Accept` header, as HTTP defines (RFC 9110, section 12):

- The device lists what it can decode (`accept_formats`, default `image/bmp, image/png, image/qoi`). Types can carry
  a weight to rank them: `image/png, image/bmp;q=0.5` asks for PNG first.
- The server sends the highest-weighted format it has. Where the client has no preference (equal weights,
  a wildcard such as `*/*` or `image/*`, or no header), `display.image_format` on the server decides.
- The reply's `Content-Type` says what was sent, and `online_image` (`format: AUTO`) picks its decoder
  from it, so the firmware never has to match the server's config. Both decoders are in the firmware.
- If none of what the client accepts exists, the reply is `406 Not Acceptable`, with the formats there are.
  The reply carries `Vary: Accept`, since it depends on the request.

Try it from a laptop:

```
curl -s -o /dev/null -w '%{content_type} %{size_download}\n' -H 'Accept: image/png' http://<host>:8080/image
curl -s -o /dev/null -w '%{http_code}\n' -H 'Accept: image/gif' http://<host>:8080/image   # 406
```

The mDNS record lists them too, for whoever is looking at a scan: `format` is what the server sends by
default, and `formats` is everything it can send (`bmp,png,qoi`). The device doesn't use either; it asks over
HTTP, which is the authoritative answer. Switching `display.image_format` on the server changes the default
with no reflash.

### Which failure it was

ESPHome gives no error type: `online_image` calls `on_error` with no arguments, and the cause is only
in the log. So the device records the stage that failed, in `fail_reason`, and each stage takes the
right path:

| Stage | `fail_reason` | What happens |
|---|---|---|
| Wi-Fi didn't connect in 30 s | `wifi` | Straight to the failure path: no `/plan`, no lookup. Notice: `No Wi-Fi @ 14:32` |
| `/plan` didn't answer, or answered badly | `server` | Look for the server once, then ask again. Notice: `Last update failed @ ...` |
| Not enough PSRAM for the image | `memory` | Checked before the download ([eink_health.h](eink_health.h)); no lookup. Notice: `Out of memory @ ...` |
| The download or decode failed after `/plan` answered | `download` | No lookup, since the server just answered. Notice: `Last update failed @ ...` |
| The wake hadn't finished after `wake_timeout` (110 s) | `timeout` | A hang, such as a server that accepts the connection and goes quiet. No notice, since the panel's state is unknown. It stands down once the image has downloaded, the wake has succeeded or it has given up for another reason. |

Four more reasons are reserved for the secure transport, which does not exist in the firmware yet. The server
already understands them (`FailureReason` in `src/application/devices.rs`, listed in `/status` and `/metrics`), and both
sides pin the same nine names in their tests:

| `fail_reason` | Meaning | Notice |
|---|---|---|
| `clock` | The clock isn't set, so no certificate can be checked and nothing that needs one is tried | `Clock not set` |
| `certificate` | A certificate was refused: the server's by the display, or the display's by the server | `Certificate refused @ ...` |
| `approval` | Asked to join; waiting for the owner to type the code in at the server | `Waiting for approval @ ...` |
| `unrecognised` | The server does not know this display's key (erased, or replaced): the owner must approve it again | `Not recognised - ask the owner to approve` |

A display that cannot join over HTTPS can still report these over plain HTTP, as the next wake that gets through,
when the server also serves plain HTTP (`prefer-https`); an HTTPS-only server shows the same thing as the display's
standing in the certificate authority.

The memory check compares the largest free PSRAM block with the decoded image (1872 x 1404 bytes) plus
96 KB, and the log says what was needed and what was free. Each failure counts towards the backoff below,
and the log line `Failure N in a row (reason)` names the stage.

### Battery

The voltage is read once at the start of every wake, before the radio or the panel are used, so it
isn't pulled down by their load. Wi-Fi is left off at boot (`enable_on_boot: false`) and switched on
afterwards, so a flat battery never starts it. Holding KEY1 switches it on at once, so an update over
the air works whatever the battery says. Three states, with hysteresis so the label doesn't flap:

| Voltage (percent) | What the device does |
|---|---|
| 3.40 V and above (10%) | Normal. |
| Below `battery_low_v` 3.40 V | Normal, but every picture carries `Battery low 8% - please charge` in the bottom-right corner. It is drawn into the same refresh as the picture, so it costs nothing extra. |
| Below `battery_empty_v` 3.30 V (5%) | Writes `Battery empty - charge to resume` once, then stops: no Wi-Fi, no refresh, and it wakes every `battery_halt_sleep_ms` (6 h) only to read the voltage. |
| Back above `battery_resume_v` 3.60 V (about 32%) | Clears both states and carries on; the picture is redrawn, which removes the notice. |

An unusable reading (under 2.5 V or over 5 V, as with no battery connected) is ignored, never a reason
to stop. Maintenance mode (KEY1 held at boot) still keeps the device awake. These thresholds are my reading
of a typical Li-ion cell, not measured on your battery: check the real voltage at the moment the
device halts, and the cell's own protection cut-off, before relying on them.

Each check-in also sends the server `device`, `battery_mv`, `battery_pct`, `battery_state`,
`failed_wakes`, `rssi` (Wi-Fi signal, dBm), `last_failure` and `last_wake_s`, which feed `/status` and
`/metrics` (see [deploy/README.md](../deploy/README.md)). [eink_telemetry.h](eink_telemetry.h) builds them.

`last_failure` is the `fail_reason` of the most recent failed wake (the table above), and `last_wake_s` is how
long the previous wake was awake. A wake asks `/plan` before it knows how it will go, so both describe the
wake *before*, and are sent by the next one that gets through. They are kept in RTC memory, which stays
powered through deep sleep, so no flash is written; a power loss forgets them, which is fine for diagnostics.
RTC memory also survives a software reset such as an update over the air, so a marker checks it was written by
this layout before it is believed. `last_failure` is left out until a wake has failed and after the next one
succeeds; `last_wake_s` is left out until there has been a wake.

### When the download fails

The old picture stays on the panel. What happens next depends on how many wakes in a row have failed
(the count is kept in flash and resets on a wake that finishes with the picture current, not merely one
where `/plan` answered):

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

## BMP, PNG or QOI

Set `display.image_format` on the server for the default; the device follows with no reflash. `Bmp` (the default) is an
8-bit greyscale file of about 2.6 MB that the firmware streams with a trivial decoder. `Png` is a small
fraction of that for a flat dashboard (about 135 KB for a real render), but the device has to inflate it.
`Qoi` is about 160 KB for the same render, a little larger than PNG, and decodes in one cheap pass with no inflate
step; it has no greyscale form, so the greys are stored as RGB and the firmware reads them back as grey. ESPHome's
`online_image` decodes all three (checked in 2026.9.1: it recognises `image/qoi`).
None has been measured on the device. To compare them, set `accept_formats` to one type (or set the server
default), run each for a day, and compare `eink_device_last_wake_seconds` in `/metrics` at a similar
`eink_device_wifi_rssi_dbm`, rather than assuming.

## Tests

The TLS side is also run against a real server on a computer, with the same mbedTLS the chip is built with:
`make -C esphome spike` (see [host/README.md](host/README.md) for what it shows and what it does not).

The calculations are kept apart from the hardware so they can be tested on a computer, with no board and no
ESPHome install, just a C++17 compiler:

```sh
make -C esphome test
```

| Header | What is in it | Tested |
|---|---|---|
| [eink_wake.h](eink_wake.h) | sleep length from the plan, the backoff, the time a wake has used, the memory the image needs | yes |
| [eink_battery.h](eink_battery.h) | charge from voltage, the low/empty latches and what to do about them | yes |
| [eink_report.h](eink_report.h) | the check-in query string, failure names, what is remembered of the last wake | yes |
| [eink_format.h](eink_format.h) | the server's URL from an address, an age in words, the notices | yes |
| [eink_service.h](eink_service.h) | which of the servers a scan found to use, and its HTTP and HTTPS ports | yes |
| [eink_pairing.h](eink_pairing.h), [eink_trust.h](eink_trust.h), [eink_sha256.h](eink_sha256.h) | the pairing code, what to do next in joining, which root to trust, where a certificate is in its life | yes |
| [eink_join.h](eink_join.h), [eink_ports.h](eink_ports.h) | one wake's joining over interfaces (the clock, the key store, the network, the chain check), run against fakes | yes |
| [eink_plan.h](eink_plan.h), [eink_clock.h](eink_clock.h) | whether to redraw after a plan; whether the clock can be believed | yes |
| [eink_state.h](eink_state.h) | the typed state one wake keeps between its scripts | no: nothing to test but the types |
| [eink_telemetry.h](eink_telemetry.h) | the radio signal and RTC memory behind the report | no: needs the chip |
| [eink_discovery.h](eink_discovery.h), [eink_health.h](eink_health.h), [eink_notice.h](eink_notice.h) | mDNS, the heap, the panel | no: need the chip |

The first four include nothing from ESPHome or ESP-IDF, and that is the rule for new calculations: put them there,
and have the YAML only read and write its globals and call them. The tests build with the address and
undefined-behaviour sanitizers, so an overflow fails the run. The YAML and the device headers are checked by
`esphome compile`, which the tests don't replace, and nothing here shows that the hardware behaves as the code
assumes (RTC memory surviving sleep, the signal reading, the panel).

The report's query string is also parsed by the server in `src/adapters/image_server/plan.rs`, from the same literal
as in `tests/report_test.cpp`, so a field renamed on one side fails a test on the other.

## Timing

The device sleeps for the `next_seconds` the server reports: the next scheduled render plus
`server.wake_delay_seconds`, so it wakes just after each render. A cron with quiet hours, such as
`*/10 6-22 * * *`, keeps it asleep overnight.

## Verify on first flash

- The log shows `3.9x V, NN%, ok` at the start of each wake, and the server's `/status` lists the device
  with the same voltage. Compare it with a multimeter on the battery once.
- To see the label and the halt without draining the battery, temporarily raise `battery_low_v` and
  `battery_empty_v` above the current voltage.

- The first boot log shows no `Not enough PSRAM` line. If it does, the figures in it say how much was
  free; check that PSRAM is detected (about 8 MB) before anything else.

- Stop the server and wake the device a few times: the label appears once, and the log shows
  `Failure 2 in a row; sleeping 1200 s`, then 2400 s. Start the server again: the next wake redraws
  the picture and the label goes.

- Hold KEY1 at boot, then let go: the log says `Maintenance mode: staying awake` and the device is still up a
  minute later, reachable for an update over the air.
- Make the server accept the connection and never answer (for example `nc -l 8080` in place of it): after
  110 s the log shows `The wake didn't finish in 110s` and `Failure 1 in a row (timeout)`, and the next
  wakes sleep 20, 40, then 60 minutes instead of 10 each time.
- A wake that downloads an image logs a sleep shorter than `next_seconds` by about the time it was awake.
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
