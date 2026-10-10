// Where the server is and how to speak to it securely, kept for the length of a wake, and the way the wake script makes
// the small requests (the plan) over it.
//
// The join (core/join.h) leaves the display with a root, a certificate and a key; the wake script then records them
// here with the server's address, and everything after goes through this: the plan directly, and the image through the
// HTTP component in esp/secure_http.h, which reads the same place. One place means the script says where the server is
// once.
//
// Written against mbedTLS 3.6 and tested on a computer against a real server (tests/tls/).
#pragma once

#include <string>
#include <utility>
#include <vector>

#include "home_display/tls/stream.h"

namespace home_display_secure {

// What is known for this wake. Not kept across sleeps: the address and ports are (core/service.h, flash), and the root
// and certificate are (core/credentials.h); this is the working copy the requests read.
struct Context {
  home_display_stream::Peer peer;
  bool ready = false;
  // How the last request through it got on, for the script to classify a failure with.
  home_display_stream::Stream::Start last = home_display_stream::Stream::Start::OK;
  // How long the first TLS handshake of the wake took (0 until there has been one), which the next wake tells the
  // server.
  uint32_t first_handshake_ms = 0;
};

// The most a request through here may take, from connecting to the last byte. The plan is the longer, since the server
// waits up to 40 s to render when the button asks for it; the picture is the one the wake's watchdog (wake.yaml) is
// behind. Both leave the watchdog's time for the rest of the wake, so a server that keeps answering a little at a time
// ends as a failed request with its reason, and not as a hung wake.
constexpr int PLAN_DEADLINE_MS = 50000;
constexpr int IMAGE_DEADLINE_MS = 45000;

inline Context &context() {
  static Context instance;
  return instance;
}

// Says where the server is and what to show it. `key` is where the key is kept (the flash identity), not a copy, and
// must stay where it is for as long as requests are made through this: the chip's identity is a static, so it does.
//
// `sessions` is where a TLS session is kept between connections, so that the plan and the picture of one wake, and the
// wakes after, resume it instead of shaking hands in full. It must be for this server and this identity
// (core/session_cache.h makes the key), and outlive the requests; null shakes hands in full every time.
inline void use(uint32_t ip, uint16_t port, const std::vector<uint8_t> &root, const std::vector<uint8_t> &certificate,
                mbedtls_pk_context *key, int timeout_ms = home_display_tls::WAIT_MS,
                home_display_session_cache::Store *sessions = nullptr) {
  Context &c = context();
  c.peer.ip = ip;
  c.peer.port = port;
  c.peer.root = root;
  c.peer.certificate = certificate;
  c.peer.key = key;
  c.peer.timeout_ms = timeout_ms;
  c.peer.deadline_ms = PLAN_DEADLINE_MS;
  c.peer.sessions = sessions;
  c.peer.parsed = root.empty() ? nullptr : home_display_stream::parse(c.peer);  // once for the wake's requests
  c.ready = ip != 0 && port != 0 && !root.empty();
  c.last = home_display_stream::Stream::Start::OK;
}

// The server's address changed (it was looked for again): keeps the rest.
inline void aim(uint32_t ip, uint16_t port) {
  Context &c = context();
  c.peer.ip = ip;
  c.peer.port = port;
  c.ready = ip != 0 && port != 0 && !c.peer.root.empty();
}

inline void forget() { context() = Context(); }

// What came of a request for something small.
struct Fetched {
  home_display_stream::Stream::Start start = home_display_stream::Stream::Start::UNREACHABLE;
  int status = 0;  // 0 when there was no answer
  std::string body;
  std::string retry_after;  // the Retry-After header, if any

  bool ok() const { return start == home_display_stream::Stream::Start::OK && status != 0; }
};

// A request whose whole answer fits in memory, up to `limit` bytes of body. `status` is the server's own, a 404 and a
// 403 included; `start` says why there is no answer when there is none.
inline Fetched fetch(const std::string &method, const std::string &path,
                     const home_display_stream::Headers &headers = {}, const std::string &content_type = "",
                     const std::string &body = "", size_t limit = 4096) {
  Fetched out;
  Context &c = context();
  if (!c.ready)
    return out;
  home_display_stream::Stream stream;
  out.start = stream.start(c.peer, method, path, headers, content_type, body);
  c.last = out.start;
  if (out.start != home_display_stream::Stream::Start::OK)
    return out;
  if (c.first_handshake_ms == 0)
    c.first_handshake_ms = stream.handshake_ms();
  out.status = stream.status();
  out.retry_after = stream.header("retry-after");
  if (!stream.read_all(out.body, limit)) {
    out.start = home_display_stream::Stream::Start::BAD_REPLY;  // cut short, or too long to be a plan
    out.status = 0;
    out.body.clear();
  }
  return out;
}

}  // namespace home_display_secure
