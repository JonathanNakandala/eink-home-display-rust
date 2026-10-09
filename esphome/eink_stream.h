// A request to the server over TLS whose answer is read as it arrives, for a body too big to hold: the image.
//
// It owns the connection, the trust anchor and the display's certificate for as long as the answer is being read, since
// the TLS session points at them. The head is parsed by eink_body.h as it comes, and the body is handed on by `read`,
// in whatever sizes the caller wants, whatever pieces the network delivers.
//
// Written against mbedTLS 3.6 and tested on a computer against a real server (host/).
#pragma once

#include <cstring>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "mbedtls/x509_crt.h"

#include "eink_body.h"
#include "eink_link.h"
#include "eink_tls.h"

namespace eink_stream {

using Bytes = eink_tls::Bytes;
using Headers = std::vector<std::pair<std::string, std::string>>;

// What the display needs to speak to the server securely: where it is, the root to check it against, and the display's
// own certificate and key to show. The key stays where it is kept (flash identity); this only points at it.
struct Peer {
  uint32_t ip = 0;  // IPv4 as lwIP stores it
  uint16_t port = 0;
  Bytes root;
  Bytes certificate;  // empty: connect without showing one
  mbedtls_pk_context *key = nullptr;
  int timeout_ms = 15000;  // for the connection, and for every wait for data after it
  // Where to find a session to resume and keep the next one (eink_session_cache.h), or null to shake hands in full
  // every time. Not owned: it outlives the stream, since what it keeps is for the next connection.
  eink_session_cache::Store *sessions = nullptr;
};

class Stream {
 public:
  Stream() = default;
  Stream(const Stream &) = delete;
  Stream &operator=(const Stream &) = delete;
  ~Stream() { close(); }

  // Why an attempt to start did not give an answer (eink_link.h).
  using Start = eink_link::Start;

  // Connects, shows the display's certificate if the peer has one, sends the request and reads as far as the end of the
  // head. The body is still to be read. `timeout_ms` of 0 is the peer's own; another one is for this request only,
  // which saves copying the peer (its root and certificate) to change it.
  Start start(const Peer &peer, const std::string &method, const std::string &path, const Headers &headers = {},
              const std::string &content_type = "", const std::string &body = "", int timeout_ms = 0) {
    close();
    mbedtls_x509_crt_init(&trust_);
    mbedtls_x509_crt_init(&own_);
    inited_ = true;
    if (mbedtls_x509_crt_parse_der(&trust_, peer.root.data(), peer.root.size()) != 0)
      return Start::UNREACHABLE;
    const bool showing = !peer.certificate.empty() && peer.key != nullptr &&
                         mbedtls_x509_crt_parse_der(&own_, peer.certificate.data(), peer.certificate.size()) == 0;
    // A saved session is tried first. If the handshake with it fails (the server forgot the ticket, was restarted with
    // other keys, or the session is from another build), it is forgotten and the connection is made again in full,
    // once. Whatever happens, the tickets the server sends are kept for the next time.
    eink_tls::Open opened = eink_tls::Open::UNREACHABLE;
    for (int attempt = 0; attempt < 2; attempt++) {
      session_.reset(new eink_tls::Session());
      session_->use_sessions(peer.sessions, attempt == 0);
      opened = session_->open(peer.ip, peer.port, &trust_, showing ? &own_ : nullptr, showing ? peer.key : nullptr,
                              timeout_ms > 0 ? timeout_ms : peer.timeout_ms);
      if (opened == eink_tls::Open::OK || attempt == 1 || peer.sessions == nullptr || !session_->offered())
        break;
      peer.sessions->forget();
    }
    if (opened == eink_tls::Open::REFUSED)
      return Start::REFUSED;
    if (opened != eink_tls::Open::OK)
      return Start::UNREACHABLE;
    if (!session_->send_request(method, path, content_type, body, headers))
      return Start::UNREACHABLE;
    // The head: raw reads, fed to the parser until it has one (anything after it is the start of the body, kept).
    uint8_t raw[512];
    while (response_.state() == eink_body::State::HEAD) {
      const int n = session_->read_some(raw, sizeof raw);
      if (n < 0)
        // TLS 1.3 tells a client whose certificate is not accepted only now, as an alert on the first read.
        return session_->refused_by_peer() ? Start::REFUSED : Start::UNREACHABLE;
      if (n == 0) {
        response_.closed();
        break;
      }
      response_.feed(raw, static_cast<size_t>(n), pending_);
    }
    return response_.state() == eink_body::State::HEAD || response_.state() == eink_body::State::FAILED
               ? Start::BAD_REPLY
               : Start::OK;
  }

  int status() const { return response_.status(); }
  // The body's length when the server gave one (the image does), else 0.
  size_t content_length() const { return response_.has_length() ? response_.content_length() : 0; }
  bool has_length() const { return response_.has_length(); }
  std::string header(const std::string &name) const { return response_.header(name); }
  const std::vector<eink_body::Header> &headers() const { return response_.headers(); }
  size_t bytes_read() const { return delivered_; }

  // True once every byte of the body has been handed on.
  bool finished() const { return response_.state() == eink_body::State::DONE && pending_.empty(); }

  // Up to `max` bytes of the body into `out`. Bytes read; 0 once the body is complete (see `finished`); negative if the
  // connection failed or the body is malformed or cut short. Waits for data for as long as the socket's timeout.
  int read(uint8_t *out, size_t max) {
    if (max == 0)
      return 0;
    while (pending_.empty()) {
      if (response_.state() == eink_body::State::DONE)
        return 0;
      if (response_.state() != eink_body::State::BODY || !session_)
        return -1;
      uint8_t raw[1024];
      const int n = session_->read_some(raw, sizeof raw);
      if (n < 0)
        return n;
      if (n == 0) {
        response_.closed();
        continue;  // DONE if the body ran to the close, else FAILED and the next turn says so
      }
      response_.feed(raw, static_cast<size_t>(n), pending_);
    }
    const size_t n = max < pending_.size() ? max : pending_.size();
    std::memcpy(out, pending_.data(), n);
    pending_.erase(0, n);
    delivered_ += n;
    return static_cast<int>(n);
  }

  // Everything that is left of the body, for a small one. False if it failed on the way.
  bool read_all(std::string &out, size_t limit) {
    uint8_t chunk[512];
    for (;;) {
      const int n = read(chunk, sizeof chunk);
      if (n < 0)
        return false;
      if (n == 0)
        return true;
      if (out.size() + static_cast<size_t>(n) > limit)
        return false;
      out.append(reinterpret_cast<char *>(chunk), static_cast<size_t>(n));
    }
  }

  void close() {
    session_.reset();
    if (inited_) {
      mbedtls_x509_crt_free(&trust_);
      mbedtls_x509_crt_free(&own_);
      inited_ = false;
    }
    response_ = eink_body::Response();
    pending_.clear();
    delivered_ = 0;
  }

 private:
  std::unique_ptr<eink_tls::Session> session_;
  mbedtls_x509_crt trust_, own_;
  bool inited_ = false;
  eink_body::Response response_;
  std::string pending_;  // body bytes decoded and not yet handed on
  size_t delivered_ = 0;
};

}  // namespace eink_stream
