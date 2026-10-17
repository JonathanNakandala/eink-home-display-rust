
# Monitoring the service

See [home-display.service](home-display.service) for running it under systemd.

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

## Looking at it in a web browser

`[server.web]` turns on a page at `/view` that shows the current picture and keeps it up to date, for a tablet on the wall or a
look from a laptop. It can be **open** to anyone who can reach the server, or need a **viewer token**, which you hand out with
`displayctl`.

```toml
[server]
transport = "prefer-https"        # tokens need HTTPS (and so the admin interface, which makes them)

[server.web]
enabled = true
# open = true                     # no sign-in at all (see below); the default is a token
layouts = ["dashboard", "departures"]   # left out or empty: all of them
# public_url = "https://display.example.home:8443"   # what the link below starts with
```

### With a token (the default)

```
displayctl -c config.toml viewer create --label "kitchen tablet" --layout departures --layout dashboard
```

prints a link like `https://display.local:8443/view#code=hdc1_…`, and, when the output is a terminal, the same link as a **QR code** to scan
with a tablet or phone instead of typing it (`--qr` to force it into a pipe, `--no-qr` to leave it out). The code is black on white in
colours of its own, so it scans on a dark theme too, and it is as secret as the link: it works once and is not shown again. Open the
link once, in the browser that is to look: the page trades
the code in it for a token the browser keeps, takes the code out of the address, and shows the picture; after that no link is
needed, on reload or tomorrow. The link works **once** and for 15 minutes (`--code-minutes`); it is not shown again. If it is
lost or not used in time, revoke the viewer and make another.

```
displayctl viewer list                    # who, where each stands (waiting, active, expired, revoked), what they may see
displayctl viewer revoke k3x6qa2m         # ends its access now: its link, or its token at its very next request
displayctl viewer forget k3x6qa2m         # removes it from the list, which frees its place (at most 64)
```

- **Refresh tokens (optional).** `--refresh` gives a viewer short-lived access tokens (an hour; `[server.web] access_token_minutes`,
  1 to 1440) and a **refresh token** that the page trades, before the access token runs out, for a new pair. A token that is lost
  or copied is then good for an hour and not for 90 days, and `--lifetime-days` is how long the *session* lasts. The refresh token
  works **once**: each use gives a new one and the old one is remembered, so if the old one is ever shown again, which only a copy
  of it could do, the whole session is ended at once (RFC 9700 section 4.14.2) and whoever comes next, the real holder
  included, needs a new link. A second use within 30 seconds is taken for a retry of a request whose answer was lost on a bad
  connection and is answered again, since ending a session for that would punish the one who is meant to have it. The page does
  all of this itself, one tab at a time (it uses the browser's Web Locks, so several tabs open on one viewer don't look like a
  copy), and renews on its own after a night asleep. `viewer list` shows a refreshing viewer's session end, and `viewer revoke`
  ends the session and its refresh token together.
- **What a viewer may see.** `--layout` (repeat it) limits a viewer to those layouts; left out, all the page may show. A token
  can't be used for more than it was made for, and `[server.web] layouts` limits the page for everyone.
- **How long.** A token lasts 90 days from first use (`--lifetime-days`, 1 to 3650), is revocable at any moment, and `viewer list`
  says when each was last used (to within ten minutes).
- **HTTPS only.** A token is a secret sent with every request, so it is only ever sent over HTTPS: the page that takes one is
  served on the HTTPS port and not on the plain one (even with `transport = "prefer-https"`), and a configuration that wants tokens
  without HTTPS, or without the admin interface to make them, is refused at start-up. Browsers warn until your authority's root
  (`root.pem` in `[server.tls] directory`) is trusted on the device.
- **What is kept.** `viewers.json` beside the list of displays holds a SHA-256 hash of each code and token, never the secret
  (both are 256 random bits, so a hash is all that is needed to recognise one and no use to anyone who reads the file).
- **Guessing.** After ten attempts from one address that were not good (a wrong code, a token that isn't one) it is told to wait
  ten minutes (`429`, with `Retry-After`); a good request clears the count. This is for noise: a 256-bit token can't be guessed.
  A source here is one address, not a whole IPv6 network, so a stale tablet asking with a revoked token turns away only itself and
  not the rest of the household.
- **Signing out.** The page's *Sign out* gives the token up (it is revoked, not only forgotten by the browser).

The exchange and the token follow the OAuth 2.0 standards, so that nothing here is invented where one exists:

| | |
|---|---|
| Sending the token | Only as `Authorization: Bearer …` (RFC 6750 section 2.1): never in an address, a form or a cookie, so there is nothing to leak into a history, a log or a referrer, and nothing for another site to make the browser send. |
| Refusing | `401` with `WWW-Authenticate: Bearer realm="home-display"`, and `error="invalid_token"` if a token was shown (RFC 6750 section 3); `403` with `error="insufficient_scope"` and the `scope` needed for a layout the token doesn't allow; `400` with `invalid_request` for a header that isn't one. |
| Getting one | `POST /view/api/token`, form-encoded, `grant_type=urn:ietf:params:oauth:grant-type:pre-authorized_code` and `pre-authorized_code=…`: the pre-authorized code grant, a code made ahead by the party that decides (you), traded once. The answer is `{"access_token", "token_type":"Bearer", "expires_in", "scope"}` with `Cache-Control: no-store` (RFC 6749 sections 4.1.3 and 5.1), and errors are `invalid_request`, `unsupported_grant_type` and `invalid_grant` (section 5.2), the last for a code that was used, ran out or never was, which are not told apart. |
| In the link | The code, after a `#`, which a browser never sends to a server (RFC 3986 section 3.5), and which the page removes from the address at once. It is one-time and short-lived, as OAuth's authorization code is, which is the reason it can be in a link and a token can't (RFC 6750 section 5.3, RFC 9700). |
| Refreshing | `grant_type=refresh_token` and `refresh_token=…` at the same endpoint (RFC 6749 section 6); the answer carries a new access token and a new refresh token. Refresh tokens rotate and are checked for reuse as RFC 9700 section 4.14 asks. They are `hdr1_…`, 256 random bits, kept only as hashes (the current one and the one before it). |
| Scope | `layout:*` or `layout:NAME` for each layout (RFC 6749 section 3.3), in the token response and in `WWW-Authenticate`. |
| The secrets | 256 random bits in base64url (RFC 4648 section 5, RFC 6819 section 5.1.4.2.2), `hdv1_` for a token and `hdc1_` for a code so that a scanner for leaked secrets can find one (RFC 8959), kept only as a hash. |
| Giving one up | `POST /view/api/revoke`, `token=…` (RFC 7009), which answers `200` whatever it is given, so it can't be used to find out which tokens exist. |
| Described | `config/openapi.json` has a `bearerAuth` scheme (optional on each route, since an open page needs none) and each route's `401`, `403` and `429`. |

Where it differs, by default: RFC 9700 prefers short-lived access tokens renewed with a rotating refresh token. For a read-only
page on a home network that is more machinery than most want, so a plain viewer has one long-lived, revocable, scoped token.
`--refresh` is the standard way, for a viewer that you want to hold to it.

### Open

With `open = true` there is no sign-in: anyone who can reach the server can see the picture, as with `/image` over plain HTTP. Keep
it on a network you trust. It is served on every listener, and with `transport = "https"` it is a way round the certificate
`/image` asks for, for the page only, so the server says so when it starts. A token is ignored.

### The page

Open `https://<server>:8443/view` (or `http://<server>:8080/view` when open). It asks the server when to look again and comes back
then, as a display does (`/plan`'s `next_seconds`), so it isn't polling on a timer of its own; it also refreshes when the tab comes
back to the front or the network returns, says how old the picture is, and says so plainly if the server has fallen behind or
can't be reached.

- **Layouts.** The bar lists the layouts the viewer may see, with **Auto**, which is whatever the layout schedule says now (a layout
  the viewer may not see is never chosen: it falls back to the first it may). Click one, press the left and right arrow keys, or
  swipe. The choice is kept in the address (`/view?layout=departures`, which is safe to bookmark or share: it has no secret in
  it) and in the browser.
- **Kiosk.** `/view#kiosk` (or `?kiosk`) hides the bar and fills the screen, with the cursor hidden: for a spare tablet. The
  "out of date" notice stays.
- **What it asks for.** `/view/api/layouts`, `/view/api/state?layout=` (the same facts as `/plan`, with nothing recorded: a browser
  never turns up among the displays) and `/view/api/image?layout=`, which is always the PNG, a fraction of the BMP a display with
  no preference is sent. They are in `config/openapi.json` under `web`.
- **Security.** The page is a single file with its script and style named by hash in a content security policy that allows nothing
  else (no inline script, no other origin, no frames; the manifest and icons are the only other things it may load, both from this
  server). All it can do is talk to this server.

### A screen of its own

By default the page shows the picture the panel gets, which is 16 greys at the panel's size (1872 by 1404 for the reTerminal), and
looks it on a colour screen. For a tablet or a monitor, give the page a screen of its own:

```toml
[server.web.screen]       # another output, like the panel: drawn at this size, in full colour
width = 1920
height = 1080
```

The page is then **another output**, beside whatever display you have (the reTerminal, the Waveshare, or none to speak of): each
run draws every layout for it too, at its own size, in full colour, with no dithering, and keeps just the PNG in
`<server.directory>/_web/<layout>/`, so the panel's pictures and the page's never meet. The layouts scale with the size they are
drawn at. It costs one more render of every layout per run (the one Chrome draws them all, whatever their sizes), but a display's button press is answered when the display's own picture is drawn, which is first, and does not wait for the page's, and the page's
pictures are reported in `/status` (`layouts[].output` is `web`) and `/metrics` (`home_display_layout_drawn{output="web",…}`) like
any other. With a Waveshare panel the server now runs for the page's sake alone (and `/healthz` and `/status` judge it by the page's pictures, since the panel, driven directly, publishes none), and a layout the page can't be given is reported
and leaves its last picture, as for any output besides the display.

### For a screen on a wall

- **Pushed, not polled.** The page opens a stream of changes (`/view/api/events`, server-sent events) and the server says the moment a
  render finishes that changes the picture, or the layout on **Auto** changes because the schedule says so, so the new picture is on
  screen about a second after it is drawn. Between, the server sends a quiet comment every 20 seconds so that nothing on the way closes
  it, and it ends the stream after 90 seconds (`event: bye`) for the page to open another at once: that keeps it under the HTTPS
  side's two-minute limit on a connection, finds a token that has been revoked within moments (the stream is ended and the next try
  refused), and means no proxy has to be waited out. The stream is read with `fetch`, because a browser's `EventSource` can't show a
  token. The page is closed while hidden to spare the battery, and if a stream can't be had at all it carries on asking as before:
  about two seconds after the next render is due (a display is told to wait thirty, for its radio and panel), and every couple of
  seconds while it is not there yet; on **Auto** it also looks again exactly when the schedule changes the layout.
- **A stream is a share of the connections, not all of them.** Each open stream holds one of the HTTPS side's 16 connections, and one
  source may hold four (a whole IPv6 network counts as one source, so every device in a home on IPv6 shares them). So only
  `[server.web] streams` (8) may be open in all and `streams_per_source` (2) from one source; one more is answered `503` with
  `Retry-After: 30`, and its page asks on a timer as above and tries for a stream again later. The rest of the connections stay for
  everything else: a page's own requests, other pages, and the displays. Plain HTTP has no connection limit of its own, so there the
  total is the only bound.
- **Without the network.** The page keeps the last picture it saw, on the device (IndexedDB), and shows it at once when it is opened
  or reloaded, saying how old it is and that it cannot reach the server; it is forgotten when the viewer signs out or its access
  ends. Where the browser has service workers (a secure page: HTTPS, or `localhost`) the page itself, its manifest and its icons are
  kept too (`/view/sw.js`, which never touches the API), so an installed page opens with the network down. The worker's cache is named
  for a fingerprint of what it keeps (the page, manifest, icons and the worker's own script), so after a server upgrade a tablet
  gets the new page, manifest and icons by itself and the old cache is deleted. A revoked token is
  refused the moment the network is back, since the API is never answered from a cache.
- **Keeping the screen on.** *Keep screen on* in the bar (and automatically in a kiosk, unless you turned it off) asks the browser
  not to let the screen sleep while the page shows, with the Screen Wake Lock. It needs a secure page: HTTPS, or `localhost`. Over
  plain HTTP the button isn't offered, and the tablet's own screen timeout applies; set it to never.
- **Installing it.** The page is a web app: *Add to home screen* (or *Install*) puts it on the tablet and opens it full screen with no
  address bar, as the kiosk (`/view#kiosk`). The icon and manifest are `/view/icon.svg`, `/view/icons/…` and
  `/view/manifest.webmanifest`, open to anyone since nothing in them is private. A viewer token is kept by the browser (and by an
  Android app installed from it); an iPhone or iPad's home-screen app keeps its own storage, so open the sign-in link inside it.
- **Locking it.** `/view#kiosk&lock` (or `?lock`) takes away the layout buttons and turns off the arrow keys and swipes, so a
  touched wall tablet stays on the layout it was set to. Pin the layout with `?layout=departures`, or give the viewer only that
  layout (`displayctl viewer create --layout departures`).
- **Still to come.** Signing a tablet in by a short code it shows (RFC 8628), for one that can't scan a QR code.

## Layouts

Each render draws every layout from the same data, so a source is fetched once however many layouts there are.
Two are built in: `dashboard` (the default: departures with the weather beside them) and `departures` (the boards
alone, in two columns and larger type).

```
curl --silent http://localhost:8080/layouts | jq        # what there is, which is the default, each one's version
curl --output out.bmp 'http://localhost:8080/image?layout=departures'
curl --silent 'http://localhost:8080/plan?layout=departures'
```

`/image`, `/plan` and `/refresh` take `layout=`; without it they answer for the default, as before. A name nothing
has is a 404 that lists the ones there are. Each layout has its own images (`<server.directory>/<layout>/image.<ext>`, and the saved screenshot `screenshot-<layout>.png`),
its own version and its own ETag. `/status` and `/healthz` judge the default layout, and a layout that fails to draw
is listed in `/status` under `layouts` (and as `home_display_layout_drawn` in `/metrics`) and makes the service `degraded`, while
the others still update and it keeps its last image. A display
driven directly (the Waveshare) shows the default layout only, so the others are not drawn for it.

### How long drawing may take

Fetching the sources and drawing the default layout have `[limits] render_deadline_seconds` (120) between them. Each other
layout has `[limits] layout_deadline_seconds` (60) of its own to be drawn and published, so more layouts don't squeeze the
run's allowance, and one that overruns is reported (the service is `degraded`) and keeps its last image while the others carry
on. Each layout is a Chrome render and an encode of every format, so on a slow host or with many layouts watch how long a run takes
(`rendering` and the log's timings) and raise the layout allowance if layouts are being given up on.

### Layouts of your own

Point `[layouts] directory` at a folder of templates (or pass `--templates-dir` to the `render` preview tool):

```toml
[layouts]
directory = "./layouts"
```

Each `NAME.html` in it is a layout called `NAME` (lower-case letters, digits and hyphens, starting with a letter). A
file named for a built-in layout, such as `dashboard.html`, replaces it and keeps its place, which is how the default is
changed; any other name adds a layout after the built-in ones. The page's `<title>` is the description `/layouts` and
`displayctl layout list` show.

A template is Handlebars, like the ones in [templates/](../templates/), which are the best examples to start from. It is
a page of exactly the panel's size, and what it can draw is what the dashboard is given:

- `date.currentTime`, `date.currentDay`, `date.currentDate`;
- `departures`: each board has `name`, `station`, `age` (set when it is from an earlier fetch), `unavailable`, `reason`
  and `services` (each with `time`, `expected`, `destination`, `countdown`, `status`); `departure_lines` is how many rows
  they need in all, for sizing the text;
- `weather_information` (absent when weather is off): `temperature`, `min`, `max`, `feels_like`, `condition`, `sun`,
  `uv_index`, `precipitation`, `air_quality`; and `weather_age`, `weather_unavailable` and `weather_reason`.

These partials are available: `{{> style}}` (the dashboard's CSS), `{{> header}}` (the clock, sun times and date),
`{{> departures}}` (the boards), and the weather icons by condition name. The Overpass fonts the built-in layouts use are
beside the page (`fonts/Overpass/...`); anything else a page needs it has to carry in itself, since it is drawn from a
temporary folder.

The folder is read once, when the program starts, and every template is drawn with sample data then (everything there,
sources down, and nothing to show). A template that isn't valid Handlebars, names a partial that doesn't exist, is not UTF-8
or is over 512 KB stops the program with the file's name and what is wrong, so a mistake shows when you restart, not at the
first render. Editing a template takes a restart. At most 16 layouts are drawn, since each costs a render on every run.

A template is code: it is drawn by Chrome from a local file, and can do anything a web page can, including reading local
files with `file://` addresses. That is fine for a folder only you can write to, which is the only kind to point this at.
Preview one first with `cargo run --bin render -- --templates-dir ./layouts`, which writes every layout, yours included.

### Layouts by time of day

`[[layouts.schedule]]` rules say which layout a display is sent at which time, for example the departures alone on weekday
mornings and the dashboard the rest of the time:

```toml
[[layouts.schedule]]
layout = "departures"
days = ["mon", "tue", "wed", "thu", "fri"]   # left out: every day
from = "06:30"                                # inclusive, 24-hour HH:MM in the server's zone
until = "09:30"                               # exclusive

[[layouts.schedule]]
layout = "clock"          # a layout of your own
from = "22:00"
until = "06:00"           # earlier than `from`: runs overnight (and `days` is the day it starts on)
```

Rules are tried in order and the first one in force wins; outside all of them the default (the first built-in layout) is
sent. A rule for a layout that doesn't exist, a time that isn't `HH:MM`, or one that starts and ends together stops the
program at start-up and says which. The times are the wall clock, so 06:30 is 06:30 either side of a clock change.

The order of precedence for what a display is sent is: `?layout=` in its own request, then the layout chosen for it with
`displayctl layout set`, then the schedule, then the default. A panel driven directly (the Waveshare) follows the schedule too:
each render draws the layout wanted at that moment. `/layouts` and `displayctl layout list` mark the layout a display with none
chosen gets right now (`current`). Every layout is still drawn on every run, so a switch costs nothing: a display gets the new
one at its first wake after the time, and is told its picture has changed so it redraws. It isn't woken at the boundary; it comes
when its own refresh says it should.

### Choosing a layout for a display

A request for a layout gets it: a display pointed at `/image?layout=departures` and `/plan?layout=departures` shows that
one. Without `layout=`, a display gets the layout the owner chose for it, or the one for the time of day if they chose none:

```
displayctl -c config.toml layout list                    # what there is
displayctl -c config.toml layout set kitchen departures  # from the kitchen display's next check-in
displayctl -c config.toml layout clear kitchen           # back to the default
displayctl -c config.toml displays show kitchen          # says "layout departures"
```

The choice is kept in `layout-choices.json` beside the list of displays (in `[server.tls] directory`), so it survives a
restart, and is shown in `displays list`, in `/status` (`members[].layout`, and `devices[].last_image.layout` for the one it was
last sent). It can be set for a display that is still waiting to be approved, and forgetting a display forgets its choice.
Like everything `displayctl` does it needs the admin socket, which the server offers when it serves HTTPS, so a display on plain
HTTP can only be pointed at a layout by its URL. (Over plain HTTP a display's name is whatever it says it is: any host on the
network can send `?device=kitchen`, and so can make the server think the kitchen display has fetched a layout it hasn't. Over
HTTPS the name is the one on the certificate, and can't be claimed.)

A display that is given another layout is told on its next `/plan` that its picture has changed, so it fetches and draws the
new one at once. (The version alone can't say so: it is the render's time in seconds, which two layouts of one run can share.)
This is judged from what the server last sent that display, so it is forgotten at a restart: a display switched while the
server was down shows the new layout from the next render. A layout that no longer exists, for a display that was given it,
is not an error: it is sent the one for the time of day.

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
| `pairings.json` | which displays are members, written whole through a temporary file and a rename, with the version before the last change kept as `pairings.json.bak`. **It holds no pairing codes**: the server works a code out only when you type one to be compared, so there is no copy in the file or its backup to read | no |

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

The server's certificate always has the name `home-display.internal`, and that is the one the display
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
- **The server's clock.** No certificate is made while the clock reads before 2026, which means it was never set
  (a machine with no real-time clock starts at 1970 until a time service sets it). That covers the displays'
  certificates *and the server's own*: with HTTPS on, the server will not start until the clock is set, and says
  why. Under the supplied unit it is started again after a pause (the unit also waits for `time-sync.target`). A
  clock that is corrected after start-up is noticed within a minute: a server certificate dated from the wrong
  time is replaced then, and until it is, the old one is kept. **The authority is held to the same rule:** a new root
  or intermediate is never made from a clock that reads before 2026 (a root dated from 1970 would end in 1990, and
  every display that pinned it would have to be paired again), so a first start with `prefer-https` or `--init-pki`
  on a machine that has not set its clock stops with a message and leaves nothing behind. Start it again once the
  clock is right.
- **A certificate never outlasts the intermediate that signed it.** One that would is cut short to the intermediate's
  end (and a warning says so), and once the intermediate has ended nothing is signed under it at all, with a message
  saying to put the root's key back and restart. This is what the warning at start-up about an intermediate near its
  end is for.
- **Limits on the settings.** `server_certificate_days` and `device_certificate_days` are from 1 to 3650, and
  `pairing_retry_minutes` from 1 to 1440. A figure outside that stops the server from starting, naming the setting.

### Sandboxing

The supplied unit confines the service: no new privileges, no capabilities, a read-only file system except the working
directory, no access to home directories beyond reading, no kernel tunables or modules, and only Unix, IPv4, IPv6 and
netlink sockets. It could not be run on the machine it was written on, so it is **untried**; treat the first start as
a test. `systemd-analyze security home-display` shows what is still open.

If it stops working after you install it:

| Symptom | Loosen |
|---|---|
| `Read-only file system` for a path you configured | add the path to `ReadWritePaths` |
| Chrome fails to start, and the log mentions the sandbox or a namespace | check `sysctl kernel.unprivileged_userns_clone` (must be 1) and, on Ubuntu 23.10 and later, the AppArmor setting `kernel.apparmor_restrict_unprivileged_userns`; neither is caused by the unit, but both look like it |
| the server won't bind its port | a port below 1024 needs `CapabilityBoundingSet=CAP_NET_BIND_SERVICE` and the same in `AmbientCapabilities` |
| a panel on the SPI bus is not found | the unit does not set `PrivateDevices`; check the user is in the `spi` and `gpio` groups |

Do not add `MemoryDenyWriteExecute`, `SystemCallFilter` or `RestrictNamespaces`: Chrome does not run under them.

### Letting a display join: `displayctl`

A display that has not joined is turned away until you open the pairing window. It is done from the machine the server
runs on, with `displayctl`, which talks to the running server over a local socket (never the network). Pairing a new
display goes like this:

1. **Open the window:** `displayctl -c config.toml window open` (15 minutes; give a number up to 240 for longer).
2. **Start the display.** It asks to join and shows a pairing code on its own panel. The server logs
   `<name> asked to join`.
3. **See who is waiting:** `displayctl -c config.toml displays list`. A display says what it is when it asks (its model, its panel
   size and greys, its firmware), so the line reads `asked 2 min ago (reTerminal E1003, 1872x1404, 16 greys, firmware 0.2.0)`:
   check that it is the display you expect before you type its code. It is the display's own word, not a check; the code is.
4. **Approve it with the code from its panel:**
   `displayctl -c config.toml approve <name> <code>`. Type the code from the panel, in any case, with or without the
   dashes; `I` and `L` are read as `1`, `O` as `0`. **The server never shows the code**, and nothing here will print
   it: a code copied from the server would say nothing about the display. If the code you type is wrong the command
   says so and nothing changes; if you are sure it is right on the panel, something may be between the display and the
   server. **Five wrong codes in a row for one display lock its approval for 15 minutes**, the right code included
   (`approve` then says `too_many_attempts` and for how long). A code that keeps being wrong is not a typing mistake, so
   the lock makes you stop and compare the panel with the server first. A right code clears the count, and a restart
   forgets it.
5. The display becomes a member the next time it asks, within a few minutes, and `displays list` then shows it as a
   `member` with when its certificate ends. You can close the window now: a display already waiting can still be
   approved. (When you have approved a new key for an existing member's name, no other key can displace it before it
   collects. If you change your mind, `reject` it.)

Other commands:

| Command | What it does |
|---|---|
| `displays list` / `displays show <name>` | every display, or one, and where each stands |
| `reject <name>` | turn a waiting display down until it is forgotten. For a member with another key waiting to take its name, turns that key down and leaves the member as it was |
| `revoke <name>` | end a member's membership: it is turned away from its very next request, whatever its certificate says |
| `forget <name>` | remove a display in any state, so it can ask again as if for the first time |

**A request nobody answers lapses after 24 hours.** A display's request to join is forgotten as if it had never asked, and
another key's request for a member's name is dropped with the member left as it was. This stops strays from filling the
(eight) places for good. A display that is still asking after that asks again, with the window open, and has a fresh day.
Members and displays you have approved never lapse.

**One address can't take all the places.** Joining is open to anyone who can reach the port while the window is open, so
each source (an address; all of an IPv6 /64 counts as one) may hold 4 connections at once and may ask to join under 4
different display names a day. A display asking again under its one name costs nothing; a fifth name is answered `429`
with `Retry-After: 600` and makes nothing wait. These are built in (`max_connections_per_source` and
`max_names_per_source` in the code) and forgotten on restart. If you are pairing several displays from one machine (a
test rig, say) and hit the limit, restart the server. Whatever a stranger asks, the answer to a refusal is the same
words, `The request was refused`; the reason (turned down, revoked, window shut) is in the log.

The socket is `admin.sock` in `[server.tls] directory`, and it is private to the user the server runs as. Two things
keep other users out, and both are checked at start-up: the socket is created with mode 0600 (macOS ignores a mode on a
socket, so there it is only a label), and **the directory it is in must be closed to everyone else**. A directory that
is not (for example one you made yourself with the default 755) stops the server from starting, and says to `chmod 700`
it. The authority's own directory is made that way.

- **Run `displayctl` as the user the server runs as,** from the server's working directory (a relative `directory` in
  the configuration is taken from where you are), or give the socket itself with `--socket PATH`.
- **It is offered only with HTTPS** (`transport = "prefer-https"` or `"https"`), because pairing is what it is for. With
  `"http"` there is no socket and nothing is written to disk.
- **A long path will not do.** A Unix socket's path is limited to about 100 bytes. If the default is too long, the server
  says so; set `[server.admin] socket` to a shorter one (in a directory only you can use).
- **Turn it off** with `[server.admin] enabled = false`. Then nothing can open the window, so no new display can join.
- **Windows:** not offered yet. The pipe it would use has not been checked to keep other users out, and that is its
  whole protection, so the server logs that it is unavailable and runs without it.
- **No secret comes back from it:** not a pairing code, a key or a certificate. You read the code off the display.
- **The API is described** in [config/admin-openapi.json](../config/admin-openapi.json) (OpenAPI 3.1), which is also served at
  `/openapi.json` on the socket. Any HTTP client that can use a Unix socket can use it, for example
  `curl --unix-socket pki/admin.sock http://localhost/v1/window`. A `PUT` needs the JSON header:
  `curl -X PUT -H 'content-type: application/json' -d '{"minutes": 15}' --unix-socket pki/admin.sock http://localhost/v1/window`.

### What to watch for in the logs

Pairing is protected by the code on the display's panel, so the log is where an attempt to interfere shows up.
**The server never prints a code.** Read it off the display's own panel and type that: a code copied from
anywhere else is the server's own, which is exactly what someone in the middle would have it show.

| Log line (level) | What it means |
|---|---|
| `<name> asked to join` (info) | A display is waiting. Check the code on its panel, then approve. |
| `The pairing code typed for <name> did not match` (warn) | A typo, or the code on the panel is not the one the server worked out. If you typed it carefully from the panel, **do not approve**: something may be between the display and the server. |
| `5 wrong pairing codes in a row for <name>: approving it is refused for 15 minutes` (warn) | Five codes were typed that were not the one the server worked out. Compare the code on the display's panel with the one you are typing before you try again: if it is right on the panel, the display may be talking to something else. |
| `<name> has not renewed its certificate, which ends in N day(s)` (warn) | A member that should have renewed by now has not (about 63 days after it was issued, for a 90-day certificate). It is off, cannot reach the server, or has a wrong clock; once it is switched on and can reach the server it gets a new one by itself. Said once a day while it stays so. |
| `<name> is a member, and another key asked for its name` (warn) | A display with an existing name asked again with a new key: a reflashed display, or someone trying to take its place. The member keeps working. Approve only if you meant to re-pair it. |
| `EST request from <address> refused: …` (warn) | Someone at that address was turned away (window shut, wrong key, not a member). A few are a display retrying; a stream from an address that is not a display is worth a look. Only 10 a minute are logged one by one; `N more refusals in the last minute were not logged one by one` says how many were left out. |
| `Turned away <name> (<address>): … not a member` (warn) | A certificate that is valid but whose display was revoked, forgotten or replaced. |
| `The intermediate certificate ends in N days …` (warn) | The root's key is not on the machine to replace it. Put it back before then. |

### Seeing trouble before it is one

`/status` lists every display's standing under `members`: its state (`pending`, `approved`, `member`, `rejected`,
`revoked`), when its latest certificate ends and how long is left, whether that has run out (`certificate_expired`,
which fixes itself when the display is next on), whether it has **stopped renewing** (`renewal_overdue`: its
certificate should have been replaced by now and has not been, about 63 days after it was issued for a 90-day
certificate; false once the certificate has ended, when `certificate_expired` says so instead), and whether a key
change or a replacement is waiting. `displayctl displays list` says it in words, and `/metrics` has the same as
`home_display_member_*`. The alert for a member that has stopped renewing:

```yaml
- alert: DisplayNotRenewing
  expr: home_display_member_certificate_renewal_overdue == 1
  for: 1h
  annotations:
    summary: "{{ $labels.device }} has not renewed its certificate when it should have"
```

A display that is simply off will trigger this, which is also the right thing to know. The log says it too, once a
day for each such display (see the table above), for those who do not run a scraper.

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
display, not the radio. Over HTTPS the server also records, for each display, how its connections began, **from its own side** (so the display
cannot misstate it): how many resumed a TLS session and how many were full handshakes, whether the latest one resumed, and
how long its handshake took, from accepting the connection to the end of it (`connection` in `/status`; `home_display_device_tls_*`
in `/metrics`). The time includes the display's own work and the network between, so a slow chip shows. The display adds
what only it knows, in the same check-in: how long its first handshake took as it timed it (`last_tls_milliseconds`) and the
least free heap it had during the wake (`last_heap_min_bytes`), which the first run on a device needs. It also says
which firmware release it runs (`firmware`, and `home_display_device_firmware_info`), and the log notes when that changes. A display that
keeps beginning with a full handshake is not resuming: look at `home_display_device_tls_last_resumed`.

The server also knows when it told the display to come back, so it can tell one that is asleep from one that
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
  - job_name: home-display
    scrape_interval: 1m
    static_configs:
      - targets: ["home-display.local:8080"]
```

```yaml
groups:
  - name: home-display
    rules:
      - alert: HomeDisplayOverdue
        expr: home_display_device_overdue == 1
        for: 5m
        annotations:
          summary: "{{ $labels.device }} has not checked in when it was told to"
      - alert: HomeDisplayBatteryLow
        expr: home_display_device_battery_state{state="low"} == 1
        for: 30m
        annotations:
          summary: "{{ $labels.device }} battery is low ({{ $value }})"
      - alert: HomeDisplayBatteryEmpty
        expr: home_display_device_battery_state{state="empty"} == 1
        annotations:
          summary: "{{ $labels.device }} has stopped refreshing: battery empty"
      - alert: HomeDisplayFailingWakes
        expr: home_display_device_failed_wakes >= 4
        annotations:
          summary: "{{ $labels.device }} has failed {{ $value }} wakes in a row (see home_display_device_last_failure)"
      - alert: HomeDisplayWeakWifi
        expr: home_display_device_wifi_rssi_dbm < -80
        for: 1h
        annotations:
          summary: "{{ $labels.device }} has a weak Wi-Fi signal ({{ $value }} dBm)"
      - alert: HomeDisplayServerStale
        expr: home_display_render_state{state="stale"} == 1
        for: 5m
        annotations:
          summary: "The dashboard is not being re-rendered"
      - alert: HomeDisplaySourceUnavailable
        expr: home_display_source_state{state="unavailable"} == 1
        for: 30m
        annotations:
          summary: "{{ $labels.source }} has had no data for half an hour"
```

A flat battery is the one failure the display cannot report itself, so `HomeDisplayOverdue` is the
alert that catches it: the display simply stops checking in. The metrics have no authentication, like
the rest of the server, so scrape from the LAN.
