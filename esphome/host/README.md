# Testing the firmware's TLS on a computer

The display's TLS is mbedTLS 3.6.x. ESPHome builds the chip's copy from the ESP-IDF in its cache, and that is plain
upstream mbedTLS, so it also builds on a computer. This directory builds that very source (`make -C esphome host-tls`,
which needs `make -C esphome compile` to have fetched it once) and runs the display's whole conversation with the
server against a **real** server, so the questions that would otherwise wait for a device are answered here.

```sh
make -C esphome spike      # needs cmake and cargo
```

The same from VS Code: the CMake panel uses `host/CMakePresets.json`, so it needs no kit. Choose the `host` preset, then
configure, build, and run the `host-tests` entry in the test panel (it runs `make host-test`). The mbedTLS source is found by
itself in ESPHome's cache, so the firmware has to have been compiled once.

`spike_tls.cpp` is the client. `run_spike.sh` starts `examples/est_fixture.rs` (the program's own server code: the
authority, EST, the admin socket, with stand-in display routes) in a fresh directory, runs the client against it, and
stops it. The client approves its own pairing with the real `displayctl`, with the code the firmware's `core/pairing.h`
works out, so a code that differs from the server's fails the spike.

## What it shows works

| Step | Result |
|---|---|
| TLS 1.3, with nothing older | works; the server offers nothing else |
| Fetch the authority over a connection that verifies nothing, and pick the self-signed root out of the CMS message | works (a 60-line DER walk; mbedTLS has no CMS) |
| Verify the server against the pinned root **and the name `eink-home-display.internal`**, while connected to `127.0.0.1` | works with `mbedtls_ssl_set_hostname`; another name, or another root, is refused |
| RFC 9266 channel binding | works: `mbedtls_ssl_export_keying_material` with label `EXPORTER-Channel-Binding` and no context (`use_context = 0`); the server accepts it and refuses a flipped one with 400 |
| The pairing code | the C++ code is approved by the real server, so the hash, the Crockford encoding and the DER public key (91 bytes) all agree with the Rust side |
| A PKCS #10 request with the binding as its challenge password | works, **built by hand**: mbedTLS's request writer cannot add a `challengePassword` attribute (only extensions), so the DER is assembled and the library signs it |
| Ask, wait (202), be approved, collect (200) | works |
| Show the certificate; the server names the display by it | works |
| Renew with the certificate shown | works, no owner |
| Resume a session | works: the server counts `resumed=1`, and the display is still known by its certificate |

## What it found that the documents did not say

1. **The chip's build had TLS 1.3 off.** `CONFIG_MBEDTLS_SSL_PROTO_TLS1_3` is not set by default, and it depends on
   `CONFIG_MBEDTLS_SSL_KEEP_PEER_CERTIFICATE`, which ESPHome turns off to save RAM. The exporter is behind
   `CONFIG_MBEDTLS_SSL_KEYING_MATERIAL_EXPORT`, also off. All three are now set in
   [packages/board.yaml](../packages/board.yaml). The static cost, from `esphome compile`: **+40 KB of flash**
   (984,779 to 1,025,151 bytes, 53.7% to 55.9%) and **+0.7 KB of RAM**. Heap during a handshake is not measured by this.
2. **A TLS 1.3 client must ask to be told about tickets.** With `mbedtls_ssl_conf_tls13_enable_signal_new_session_tickets`,
   `mbedtls_ssl_read` returns `MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET`, and the session has to be taken then
   (`mbedtls_ssl_get_session`); without it the ticket is thrown away and nothing resumes. The first spike run had
   no tickets for this reason.
3. **An enrolment answer carries the display's certificate alone.** It does not chain to the root by itself; it needs the
   intermediate, which the server presents in the TLS handshake (`mbedtls_ssl_get_peer_cert`, its `next`). So the display
   does need the intermediate once, to check a certificate it was just given, and takes it from the connection. The
   `Est` interface returns it as `Reply::intermediates` and `Verifier::chains_to` takes it.
4. **Verification of the name is separate from the address**, as the README hoped, so `eink-home-display.internal` works
   with whatever address mDNS found.

## What it does not show

- **Heap and speed on the chip.** This is the same library with upstream's defaults and a computer's memory. The chip
  has hardware acceleration, smaller buffers and less than 340 KB of RAM. How much heap a TLS 1.3 handshake with a
  client certificate needs there, and how long it takes, needs a device.
- **Flash, power cuts, the radio, the panel.**
- That the chip's configuration (`sdkconfig`) and this directory's (`mbedtls_host_config.h`) agree on every option: the
  host one starts from upstream's defaults and adds only the exporter.

## The firmware's own code, against the real server

`make -C esphome host-test` runs `join_host_test.cpp`: **the firmware's joining logic (`core/join.h`) driving the
firmware's own TLS and EST code (`tls/est_client.h`, `tls/tls.h`, `tls/csr.h`, `tls/verifier.h`) against the real
server**, with a display's identity held in memory (`memory_identity.h`) and a clock a test can move. Only the flash
and the radio differ from the chip. Each test starts its own server, since the server limits how many different names
one address may ask to join with in a day.

| Test | Shows |
|---|---|
| a new display waits, shows the code the server approves, and is then paired | the whole first join; the root is kept only after the certificate; the certificate is dated an hour back and 90 days on |
| a wrong code is not approved | the server refuses a code that is not the one worked out from the display's own root and key |
| a paired display goes on without asking the network anything | no request at all on an ordinary wake |
| with a third of the certificate left it renews with no owner | `simplereenroll` over a connection that shows the certificate; a new certificate |
| a certificate that ended is asked for again with the same key and no owner | `simpleenroll` with the key it holds, no root fetched |
| a server that does not chain to the pinned root is refused | a root of someone else's: refused at the handshake, reported as `certificate`, nothing sent |
| a server that is not there is reported as the server | nothing listening: `server`, no key made, nothing remembered |
| a display the owner has revoked is not kept but loses nothing | the renewal is refused, it asks as a new display and is refused that too: not recognised, with the code to give the owner; key, certificate and root all still held |
| once the owner approves it again a revoked display is back | revoke, forget, ask, approve the new code, paired |

Four more use `tls/flash_identity.h` over a flash stand-in, with a display that starts afresh at every wake (as after
deep sleep) and keeps nothing between wakes but its flash and the one value RTC memory holds:

| Test | Shows |
|---|---|
| a display that starts afresh at every wake joins and renews from what flash holds | the key made at the first wake is the one used at the second; an ordinary wake asks nothing; renewal keeps the key |
| a display that lost what the server last said just asks again | RTC memory is lost on a power cut; the same key gives the same code and the server says wait again |
| a power cut while the certificate is written leaves the display as it was | the next write is cut short and the power goes: on the next start the record before it is read, whole, and the display renews |
| flash that cannot be read is left alone and the server is not asked | both slots fail to read: reported as memory, nothing written, no request; when the flash comes back it is the same display |

Eleven more are about resuming a TLS session on the data connections (`resume_host_test.cpp`, and one in
`begin_host_test.cpp`), which saves a handshake's certificates, signatures and about two thirds of its bytes:

| Test | Shows |
|---|---|
| the second connection of a wake resumes the first | the plan leaves a ticket, the picture and the next request resume it, and the server still names the display by the certificate of the first (the server counts `resumed`) |
| a session survives being written out and read back as after deep sleep | the slot (RTC memory on the chip) and the wall clock are all a woken program has, and that is enough |
| a ticket whose clock started again is still accepted because its age is put right | **see below**: without the fix the ticket is refused without a word |
| a ticket the server does not know is a full handshake, not a failed request | a damaged ticket, as after the server was restarted with other keys: the connection is made in full, and the next one resumes the new ticket |
| a session the library cannot read is forgotten | a slot from another build: forgotten, connection in full |
| a renewed certificate does not use the session of the one before | the cache key includes the certificate, the root, the address and the port |
| sessions are not shared between displays | another display handed this one's memory is offered nothing |
| a display the owner revoked is still turned away on a resumed connection | the connection resumes, the request is a 403: membership is checked on every request |
| with no store every connection is in full; with the clock not believed nothing is kept or offered | the two off switches |
| what a session costs | **698 bytes** saved, in a slot of 3 KB |
| a wake that begins with a session store | the plan and the picture of a wake, and the wake after a sleep, resume (the server counts three) |

**The clock mbedTLS dates a ticket by restarts at deep sleep.** A TLS 1.3 client works out how old a ticket is from
`mbedtls_ms_time()`, which on the chip is `CLOCK_MONOTONIC` (`esp_platform_time.c`): it starts again from nothing when the chip
wakes. A session saved before a sleep and loaded after one would have a reception time that is in the future, or far in the
past, and `ssl_prepare_client_hello` drops it with "ticket expired, disable session resumption": the connection is made in
full, and nothing says why. So the cache keeps the **wall-clock** time the session was saved at, and `Session::restore` sets
the ticket's reception time to `now - (wall now - saved at)` after loading it. The test above makes a session as if from a clock an
hour ahead, shows that mbedTLS would see it as from the future, and shows that it resumes through the cache. This is why the
cache needs SNTP: with no clock that can be believed, nothing is kept and nothing is offered.

Run one test with `host/with_fixture.sh build/host/join_host_test <name>`; `--list` prints the names.

One thing these cannot do is move the server's clock, so a test that puts the display's clock ahead sees the
server's real-time certificates as already over by it. The expired-certificate test holds the display's dates
back instead.
