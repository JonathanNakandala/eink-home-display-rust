// Where the server is and how to speak to it securely, kept for the length of a wake, and the way the wake script makes
// the small requests (the plan) over it.
//
// The join (eink_join.h) leaves the display with a root, a certificate and a key; the wake script then records them
// here with the server's address, and everything after goes through this: the plan directly, and the image through the
// HTTP component in eink_secure_http.h, which reads the same place. One place means the script says where the server is
// once.
//
// Written against mbedTLS 3.6 and tested on a computer against a real server (host/).
#pragma once

#include <string>
#include <utility>
#include <vector>

#include "eink_stream.h"

namespace eink_secure {

// What is known for this wake. Not kept across sleeps: the address and ports are (eink_service.h, flash), and the root
// and certificate are (eink_credentials.h); this is the working copy the requests read.
struct Context {
  eink_stream::Peer peer;
  bool ready = false;
  // How the last request through it got on, for the script to classify a failure with.
  eink_stream::Stream::Start last = eink_stream::Stream::Start::OK;
};

inline Context &context() {
  static Context instance;
  return instance;
}

// Says where the server is and what to show it. `key` is where the key is kept (the flash identity), not a copy, and
// must stay where it is for as long as requests are made through this: the chip's identity is a static, so it does.
//
// `sessions` is where a TLS session is kept between connections, so that the plan and the picture of one wake, and the
// wakes after, resume it instead of shaking hands in full. It must be for this server and this identity
// (eink_session_cache.h makes the key), and outlive the requests; null shakes hands in full every time.
inline void use(uint32_t ip, uint16_t port, const std::vector<uint8_t> &root, const std::vector<uint8_t> &certificate,
                mbedtls_pk_context *key, int timeout_ms = 15000, eink_session_cache::Store *sessions = nullptr) {
  Context &c = context();
  c.peer.ip = ip;
  c.peer.port = port;
  c.peer.root = root;
  c.peer.certificate = certificate;
  c.peer.key = key;
  c.peer.timeout_ms = timeout_ms;
  c.peer.sessions = sessions;
  c.ready = ip != 0 && port != 0 && !root.empty();
  c.last = eink_stream::Stream::Start::OK;
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
  eink_stream::Stream::Start start = eink_stream::Stream::Start::UNREACHABLE;
  int status = 0;  // 0 when there was no answer
  std::string body;
  std::string retry_after;  // the Retry-After header, if any

  bool ok() const { return start == eink_stream::Stream::Start::OK && status != 0; }
};

// A request whose whole answer fits in memory, up to `limit` bytes of body. `status` is the server's own, a 404 and a
// 403 included; `start` says why there is no answer when there is none.
inline Fetched fetch(const std::string &method, const std::string &path, const eink_stream::Headers &headers = {},
                     const std::string &content_type = "", const std::string &body = "", size_t limit = 4096) {
  Fetched out;
  Context &c = context();
  if (!c.ready)
    return out;
  eink_stream::Stream stream;
  out.start = stream.start(c.peer, method, path, headers, content_type, body);
  c.last = out.start;
  if (out.start != eink_stream::Stream::Start::OK)
    return out;
  out.status = stream.status();
  out.retry_after = stream.header("retry-after");
  if (!stream.read_all(out.body, limit)) {
    out.start = eink_stream::Stream::Start::BAD_REPLY;  // cut short, or too long to be a plan
    out.status = 0;
    out.body.clear();
  }
  return out;
}

}  // namespace eink_secure
