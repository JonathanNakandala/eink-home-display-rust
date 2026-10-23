# Security policy

## Reporting a problem

Please report a vulnerability **privately**, not in a public issue: on this repository's **Security** tab, choose **Report a
vulnerability** (GitHub's private reporting). Say what you found, what it needs (network access, a paired display, a local user),
and how to see it. This is a personal project, so a reply is on a best-effort basis, but a report is read and is not made public
before it is fixed.

## What is supported

The latest release of the server and of the firmware. Fixes are made in a new release; older ones are not patched.

## What to know about how it is meant to be used

- **The server is for a home network.** Its pages (`/image`, `/plan`, `/status`, `/healthz`, `/metrics`) have no password, by design,
  like the picture itself, and `/status` and errors can name a failing data source. Do not put its ports on the internet. With
  `transport = "https"` only a display that has joined is served; with `prefer-https` plain HTTP is served too.
- **The certificate authority's root key is the one secret that cannot be replaced.** It is in `/var/lib/home-display/pki/` (readable
  by the service's user alone) and in backups, which should be encrypted if they leave the machine (INSTALL.md, "Back up").
- **Joining a display is approved by the owner**, at the server, with a short code shown on the display, and is rate limited. The admin
  interface (`displayctl`) is a local socket, never the network.
- **API keys** you give the server (weather, rail data) are kept in memory, or in `credentials.json` in plain text, private to its user,
  if you ask for them to be remembered.

A report that the server can be reached by anyone on the network it is on, without a password, is therefore not a vulnerability. One
that gets round the pairing, reads what a display should not, reads the keys, or runs something on the machine is.

## Dependencies

`cargo-deny` checks the dependencies for known vulnerabilities on every change to `Cargo.lock` and every week (`audit.yml`), with what is
ignored, and why, in [deny.toml](deny.toml).
