// A TLS 1.3 connection to the server with mbedTLS, over BSD sockets (which lwIP has too), one HTTP request on it, and
// the things the display takes from the connection: the RFC 9266 channel binding and what the server presented.
//
// Written against mbedTLS 3.6 and tested on a computer against a real server (host/), with the same source the chip is
// built from. Nothing here throws or needs RTTI, since the chip's build has neither.
#pragma once

#include <cerrno>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

#include <fcntl.h>
#include <netinet/in.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

#include "mbedtls/ctr_drbg.h"
#include "mbedtls/entropy.h"
#include "mbedtls/error.h"
#include "mbedtls/pk.h"
#include "mbedtls/platform_time.h"
#include "mbedtls/ssl.h"
#include "mbedtls/x509_crt.h"

#include "eink_http.h"
#include "eink_ports.h"
#include "eink_session_cache.h"

namespace eink_tls {

using Bytes = eink_ports::Bytes;

// The name the server's certificate always has, and the only one a display checks, whatever address mDNS found.
constexpr const char *SERVER_NAME = "eink-home-display.internal";

// The random numbers for keys, handshakes and signatures. One for the program.
class Random {
 public:
  static Random &get() {
    static Random instance;
    return instance;
  }
  static int generate(void *, unsigned char *out, size_t length) {
    return mbedtls_ctr_drbg_random(&get().drbg_, out, length);
  }

 private:
  Random() {
    mbedtls_entropy_init(&entropy_);
    mbedtls_ctr_drbg_init(&drbg_);
    mbedtls_ctr_drbg_seed(&drbg_, mbedtls_entropy_func, &entropy_, nullptr, 0);
  }
  mbedtls_entropy_context entropy_;
  mbedtls_ctr_drbg_context drbg_;
};

// Where an attempt to open a connection got to.
enum class Open : uint8_t {
  OK,
  UNREACHABLE,  // no connection, no answer, or a handshake that failed for any other reason
  REFUSED,      // a certificate was refused: the server's by us, or ours by the server
};

// The display's key in the form TLS wants it. Added to the interface the joining logic sees, which is all about bytes.
class TlsIdentity : public eink_ports::Identity {
 public:
  virtual mbedtls_pk_context &private_key() = 0;
};

class Session {
 public:
  Session() {
    mbedtls_ssl_init(&ssl_);
    mbedtls_ssl_config_init(&conf_);
  }
  ~Session() {
    mbedtls_ssl_free(&ssl_);
    if (have_offered_session_)
      mbedtls_ssl_session_free(&offered_session_);
    mbedtls_ssl_config_free(&conf_);
    if (socket_ >= 0)
      ::close(socket_);
  }
  Session(const Session &) = delete;
  Session &operator=(const Session &) = delete;

  // Resumes an earlier session, and keeps the tickets the server sends for the next one. For connections that show a
  // certificate and verify the server, never for the first contact or the enrolment, which are made fresh each time, so
  // that each of them is tied to its own connection (RFC 9266) and nothing about a display is kept before the owner has
  // approved it.
  //   `offer` says whether to try the saved session; false still keeps new tickets (the saved one just failed).
  // Call before `open`.
  void use_sessions(eink_session_cache::Store *store, bool offer = true) {
    sessions_ = store;
    offer_ = offer;
  }

  // The saved session was offered to the server on this connection.
  bool offered() const { return offered_; }

  // The session as bytes, to keep: mbedTLS's own serialisation, which holds the ticket and the secrets that use it, so
  // it is as secret as the key. Empty if it cannot be had.
  static Bytes serialise(const mbedtls_ssl_session &session) {
    size_t length = 0;
    mbedtls_ssl_session_save(&session, nullptr, 0, &length);
    if (length == 0)
      return {};
    Bytes out(length);
    if (mbedtls_ssl_session_save(&session, out.data(), out.size(), &length) != 0)
      return {};
    out.resize(length);
    return out;
  }

  // A saved session made ready to offer: loaded, and its age put right. mbedTLS dates a ticket by a monotonic clock
  // (`mbedtls_ms_time`), which starts again when the chip wakes from deep sleep, so a ticket saved before the sleep
  // would look as if it came from the future, or from long ago, and be refused without a word. `age_s` is how long it
  // has really been, from the wall clock, and the ticket's reception time is set so that mbedTLS sees that age now.
  // False if it cannot be loaded (it was saved by another build of mbedTLS, whose format differs).
  static bool restore(const Bytes &saved, int64_t age_s, mbedtls_ssl_session &session) {
    if (saved.empty() || age_s < 0 || mbedtls_ssl_session_load(&session, saved.data(), saved.size()) != 0)
      return false;
    session.MBEDTLS_PRIVATE(ticket_reception_time) = mbedtls_ms_time() - age_s * 1000;
    return true;
  }

  // Connects to `ip` (IPv4 as lwIP stores it: the first octet in the lowest byte) and shakes hands, TLS 1.3 only.
  // `trust` null verifies nothing (first contact, to fetch the root, which the pairing code confirms later); otherwise
  // the server must chain to it and be named SERVER_NAME. `own_certificate` and `own_key` are shown if both are given.
  Open open(uint32_t ip, uint16_t port, const mbedtls_x509_crt *trust, const mbedtls_x509_crt *own_certificate,
            mbedtls_pk_context *own_key, int timeout_ms = 15000) {
    if (!connect_socket(ip, port, timeout_ms))
      return Open::UNREACHABLE;
    if (mbedtls_ssl_config_defaults(&conf_, MBEDTLS_SSL_IS_CLIENT, MBEDTLS_SSL_TRANSPORT_STREAM,
                                    MBEDTLS_SSL_PRESET_DEFAULT) != 0)
      return Open::UNREACHABLE;
    mbedtls_ssl_conf_min_tls_version(&conf_, MBEDTLS_SSL_VERSION_TLS1_3);
    mbedtls_ssl_conf_max_tls_version(&conf_, MBEDTLS_SSL_VERSION_TLS1_3);
    mbedtls_ssl_conf_rng(&conf_, Random::generate, nullptr);
    if (trust != nullptr) {
      mbedtls_ssl_conf_authmode(&conf_, MBEDTLS_SSL_VERIFY_REQUIRED);
      mbedtls_ssl_conf_ca_chain(&conf_, const_cast<mbedtls_x509_crt *>(trust), nullptr);
    } else {
      mbedtls_ssl_conf_authmode(&conf_, MBEDTLS_SSL_VERIFY_NONE);
    }
    if (own_certificate != nullptr && own_key != nullptr)
      mbedtls_ssl_conf_own_cert(&conf_, const_cast<mbedtls_x509_crt *>(own_certificate), own_key);
    if (mbedtls_ssl_setup(&ssl_, &conf_) != 0)
      return Open::UNREACHABLE;
    // The name the certificate must have, whatever address was connected to.
    if (trust != nullptr && mbedtls_ssl_set_hostname(&ssl_, SERVER_NAME) != 0)
      return Open::UNREACHABLE;
    if (sessions_ != nullptr) {
      // In TLS 1.3 a ticket arrives after the handshake; the library keeps it only if the application asks to be told.
      mbedtls_ssl_conf_tls13_enable_signal_new_session_tickets(&conf_,
                                                               MBEDTLS_SSL_TLS1_3_SIGNAL_NEW_SESSION_TICKETS_ENABLED);
      Bytes saved;
      int64_t age_s = 0;
      if (offer_ && sessions_->load(saved, age_s)) {
        mbedtls_ssl_session_init(&offered_session_);
        have_offered_session_ = true;
        if (restore(saved, age_s, offered_session_) && mbedtls_ssl_set_session(&ssl_, &offered_session_) == 0)
          offered_ = true;
        else
          sessions_->forget();  // it cannot be used (another build wrote it): not kept to fail every wake
      }
    }
    mbedtls_ssl_set_bio(&ssl_, this, &Session::send_bytes, &Session::receive_bytes, nullptr);
    int r;
    while ((r = mbedtls_ssl_handshake(&ssl_)) == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE) {
    }
    error_ = r;
    if (r == 0)
      return Open::OK;
    return mbedtls_ssl_get_verify_result(&ssl_) != 0 || r == MBEDTLS_ERR_X509_CERT_VERIFY_FAILED ||
                   r == MBEDTLS_ERR_SSL_FATAL_ALERT_MESSAGE
               ? Open::REFUSED
               : Open::UNREACHABLE;
  }

  // One request and everything the server answers, which it then closes the connection on. In TLS 1.3 the server tells
  // a client whose certificate it does not accept only after the handshake, as an alert on the first read: then
  // `refused_by_peer()` is true and the response has no status.
  eink_http::Response request(const std::string &method, const std::string &path, const std::string &content_type,
                              const std::string &body) {
    const std::string out = eink_http::request(method, path, SERVER_NAME, content_type, body);
    size_t sent = 0;
    while (sent < out.size()) {
      const int r =
          mbedtls_ssl_write(&ssl_, reinterpret_cast<const unsigned char *>(out.data()) + sent, out.size() - sent);
      if (r == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE)
        continue;
      if (r < 0) {
        error_ = r;
        return {};
      }
      sent += static_cast<size_t>(r);
    }
    std::string raw;
    unsigned char buffer[1024];
    for (;;) {
      const int r = mbedtls_ssl_read(&ssl_, buffer, sizeof buffer);
      if (r == MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET) {
        keep_ticket();
        continue;
      }
      if (r == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE)
        continue;
      if (r <= 0) {
        error_ = r;
        break;  // closed, cleanly or not: what arrived is the answer
      }
      raw.append(reinterpret_cast<char *>(buffer), static_cast<size_t>(r));
    }
    return eink_http::parse(raw);
  }

  // Sends a request and does not read the answer, for a body that is read as it comes (`read_some`). False if it could
  // not be sent.
  bool send_request(const std::string &method, const std::string &path, const std::string &content_type,
                    const std::string &body, const std::vector<std::pair<std::string, std::string>> &headers = {}) {
    const std::string out = eink_http::request(method, path, SERVER_NAME, content_type, body, headers);
    size_t sent = 0;
    while (sent < out.size()) {
      const int r =
          mbedtls_ssl_write(&ssl_, reinterpret_cast<const unsigned char *>(out.data()) + sent, out.size() - sent);
      if (r == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE)
        continue;
      if (r < 0) {
        error_ = r;
        return false;
      }
      sent += static_cast<size_t>(r);
    }
    return true;
  }

  // Up to `length` bytes of the answer, waiting for them for as long as the socket's timeout. A positive number is
  // bytes read, 0 is the server closing the connection cleanly, and a negative one is an error (a timeout, a reset, an
  // alert).
  int read_some(uint8_t *out, size_t length) {
    for (;;) {
      const int r = mbedtls_ssl_read(&ssl_, out, length);
      if (r == MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET) {
        keep_ticket();
        continue;
      }
      if (r == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE)
        continue;
      if (r == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY)
        return 0;
      if (r < 0)
        error_ = r;
      return r;
    }
  }

  // The 32 bytes RFC 9266 binds a request to its connection with (tls-exporter): empty if the library cannot make them.
  Bytes channel_binding() {
    Bytes out(32);
    const char *label = "EXPORTER-Channel-Binding";
    if (mbedtls_ssl_export_keying_material(&ssl_, out.data(), out.size(), label, std::strlen(label), nullptr, 0, 0) !=
        0)
      return {};
    return out;
  }

  // What the server presented beyond its own certificate: its intermediates, each as DER.
  std::vector<Bytes> intermediates() const {
    std::vector<Bytes> out;
    const mbedtls_x509_crt *peer = mbedtls_ssl_get_peer_cert(&ssl_);
    for (const mbedtls_x509_crt *c = peer != nullptr ? peer->next : nullptr; c != nullptr; c = c->next)
      out.emplace_back(c->raw.p, c->raw.p + c->raw.len);
    return out;
  }

  // The server turned our certificate down (TLS 1.3 says so after the handshake, as an alert).
  bool refused_by_peer() const { return error_ == MBEDTLS_ERR_SSL_FATAL_ALERT_MESSAGE; }
  int error() const { return error_; }

 private:
  int socket_ = -1;
  int timeout_ms_ = 15000;
  int error_ = 0;
  mbedtls_ssl_context ssl_;
  mbedtls_ssl_config conf_;
  eink_session_cache::Store *sessions_ = nullptr;
  bool offer_ = true;
  bool offered_ = false;
  mbedtls_ssl_session offered_session_;
  bool have_offered_session_ = false;

  // A ticket has arrived: the session as it is now is the one to resume next time.
  void keep_ticket() {
    if (sessions_ == nullptr)
      return;
    mbedtls_ssl_session session;
    mbedtls_ssl_session_init(&session);
    if (mbedtls_ssl_get_session(&ssl_, &session) == 0) {
      const Bytes bytes = serialise(session);
      if (!bytes.empty())
        sessions_->save(bytes);
    }
    mbedtls_ssl_session_free(&session);
  }

  bool connect_socket(uint32_t ip, uint16_t port, int timeout_ms) {
    timeout_ms_ = timeout_ms;
    socket_ = ::socket(AF_INET, SOCK_STREAM, 0);
    if (socket_ < 0)
      return false;
    struct sockaddr_in address;
    std::memset(&address, 0, sizeof address);
    address.sin_family = AF_INET;
    address.sin_port = htons(port);
    address.sin_addr.s_addr = ip;  // already in network order: the first octet is the lowest byte
    // A connect that gives up after a time of our choosing, and not the stack's: non-blocking, then wait for it.
    const int flags = ::fcntl(socket_, F_GETFL, 0);
    ::fcntl(socket_, F_SETFL, flags | O_NONBLOCK);
    int r = ::connect(socket_, reinterpret_cast<struct sockaddr *>(&address), sizeof address);
    if (r < 0 && errno == EINPROGRESS) {
      fd_set writable;
      FD_ZERO(&writable);
      FD_SET(socket_, &writable);
      struct timeval wait;
      wait.tv_sec = timeout_ms / 1000;
      wait.tv_usec = (timeout_ms % 1000) * 1000;
      if (::select(socket_ + 1, nullptr, &writable, nullptr, &wait) <= 0)
        return false;
      int failure = 0;
      socklen_t size = sizeof failure;
      if (::getsockopt(socket_, SOL_SOCKET, SO_ERROR, &failure, &size) < 0 || failure != 0)
        return false;
      r = 0;
    }
    if (r < 0)
      return false;
    ::fcntl(socket_, F_SETFL, flags);
    struct timeval limit;
    limit.tv_sec = timeout_ms / 1000;
    limit.tv_usec = (timeout_ms % 1000) * 1000;
    ::setsockopt(socket_, SOL_SOCKET, SO_RCVTIMEO, &limit, sizeof limit);
    ::setsockopt(socket_, SOL_SOCKET, SO_SNDTIMEO, &limit, sizeof limit);
    return true;
  }

  static int send_bytes(void *context, const unsigned char *data, size_t length) {
    const Session *self = static_cast<const Session *>(context);
    const ssize_t n = ::send(self->socket_, data, length, 0);
    if (n >= 0)
      return static_cast<int>(n);
    return errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR ? MBEDTLS_ERR_SSL_WANT_WRITE
                                                                     : MBEDTLS_ERR_SSL_INTERNAL_ERROR;
  }

  static int receive_bytes(void *context, unsigned char *data, size_t length) {
    const Session *self = static_cast<const Session *>(context);
    const ssize_t n = ::recv(self->socket_, data, length, 0);
    if (n >= 0)
      return static_cast<int>(n);  // 0 is the other end closing
    if (errno == EINTR)
      return MBEDTLS_ERR_SSL_WANT_READ;
    // SO_RCVTIMEO has already waited the whole time when this is EAGAIN: that is a timeout, not "try again".
    return errno == EAGAIN || errno == EWOULDBLOCK ? MBEDTLS_ERR_SSL_TIMEOUT : MBEDTLS_ERR_SSL_INTERNAL_ERROR;
  }
};

}  // namespace eink_tls
