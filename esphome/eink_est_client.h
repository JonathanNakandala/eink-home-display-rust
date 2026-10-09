// The display's side of EST (RFC 7030 as corrected by RFC 8951) over eink_tls.h: fetch the authority, ask for a
// certificate, renew one. It implements the `Est` interface eink_join.h drives, and is run against a real server on a
// computer (host/).
#pragma once

#include <string>
#include <vector>

#include "mbedtls/x509_crt.h"

#include "eink_base64.h"
#include "eink_calendar.h"
#include "eink_csr.h"
#include "eink_der.h"
#include "eink_http.h"
#include "eink_ports.h"
#include "eink_tls.h"

namespace eink_est {

using Bytes = eink_tls::Bytes;

// An mbedTLS date and time as seconds since 1970.
inline int64_t epoch(const mbedtls_x509_time &t) {
  return eink_calendar::epoch_seconds(t.year, t.mon, t.day, t.hour, t.min, t.sec);
}

class EstClient : public eink_ports::Est {
 public:
  // `ip` is the server's IPv4 as lwIP stores it, and `port` its HTTPS port (eink_service.h). `identity` supplies the
  // key for the requests and for showing the certificate.
  EstClient(uint32_t ip, uint16_t port, eink_tls::TlsIdentity &identity) : ip_(ip), port_(port), identity_(identity) {}

  // GET cacerts over a connection that verifies nothing, and the self-signed certificate in it.
  eink_ports::RootReply fetch_root() override {
    eink_ports::RootReply reply;
    eink_tls::Session session;
    if (session.open(ip_, port_, nullptr, nullptr, nullptr) != eink_tls::Open::OK)
      return reply;  // unreachable
    const eink_http::Response response = session.request("GET", path("cacerts"), "", "");
    if (response.status != 200)
      return reply;
    reply.result = eink_ports::Fetch::BAD;
    Bytes cms;
    if (!eink_base64::decode(response.body, cms))
      return reply;
    for (const Bytes &der : eink_der::certificates_in(cms)) {
      if (is_root(der)) {
        reply.result = eink_ports::Fetch::OK;
        reply.root = der;
        break;
      }
    }
    return reply;
  }

  eink_ports::Reply enroll(const std::string &name, eink_ports::Identity &, const Bytes &root) override {
    return post("simpleenroll", name, root, false);
  }

  eink_ports::Reply renew(const std::string &name, eink_ports::Identity &, const Bytes &root) override {
    return post("simplereenroll", name, root, true);
  }

 private:
  uint32_t ip_;
  uint16_t port_;
  eink_tls::TlsIdentity &identity_;

  static std::string path(const char *operation) { return std::string("/.well-known/est/") + operation; }

  // A certificate that names itself as its issuer and is signed by its own key.
  static bool is_root(const Bytes &der) {
    mbedtls_x509_crt crt;
    mbedtls_x509_crt_init(&crt);
    bool root = mbedtls_x509_crt_parse_der(&crt, der.data(), der.size()) == 0 &&
                crt.subject_raw.len == crt.issuer_raw.len &&
                std::memcmp(crt.subject_raw.p, crt.issuer_raw.p, crt.subject_raw.len) == 0;
    if (root) {
      uint32_t flags = 0;
      root = mbedtls_x509_crt_verify(&crt, &crt, nullptr, nullptr, &flags, nullptr, nullptr) == 0;
    }
    mbedtls_x509_crt_free(&crt);
    return root;
  }

  eink_ports::Reply post(const char *operation, const std::string &name, const Bytes &root, bool show_certificate) {
    eink_ports::Reply reply;  // UNREACHABLE until something better is known
    mbedtls_x509_crt trust, own;
    mbedtls_x509_crt_init(&trust);
    mbedtls_x509_crt_init(&own);
    if (mbedtls_x509_crt_parse_der(&trust, root.data(), root.size()) != 0) {
      mbedtls_x509_crt_free(&trust);
      return reply;
    }
    const Bytes certificate = identity_.held_certificate();
    const bool showing = show_certificate && !certificate.empty() &&
                         mbedtls_x509_crt_parse_der(&own, certificate.data(), certificate.size()) == 0;
    {
      eink_tls::Session session;
      const eink_tls::Open opened =
          session.open(ip_, port_, &trust, showing ? &own : nullptr, showing ? &identity_.private_key() : nullptr);
      if (opened == eink_tls::Open::REFUSED) {
        reply.result = eink_ports::Result::TLS_REFUSED;
      } else if (opened == eink_tls::Open::OK) {
        // Made on this connection, so it can only be sent on this one.
        const Bytes request = identity_.csr(name, session.channel_binding());
        const eink_http::Response response =
            request.empty()
                ? eink_http::Response()
                : session.request("POST", path(operation), "application/pkcs10", eink_base64::encode(request));
        if (session.refused_by_peer())
          reply.result = eink_ports::Result::TLS_REFUSED;
        else
          fill(reply, response, session);
      }
    }
    mbedtls_x509_crt_free(&trust);
    mbedtls_x509_crt_free(&own);
    return reply;
  }

  static void fill(eink_ports::Reply &reply, const eink_http::Response &response, const eink_tls::Session &session) {
    if (response.status == 202) {
      reply.result = eink_ports::Result::PENDING;
      reply.retry_after_s =
          static_cast<uint32_t>(std::strtoul(eink_http::header(response.headers, "retry-after").c_str(), nullptr, 10));
    } else if (response.status == 403) {
      reply.result = eink_ports::Result::REFUSED;
    } else if (response.status == 200) {
      Bytes cms;
      const std::vector<Bytes> certificates =
          eink_base64::decode(response.body, cms) ? eink_der::certificates_in(cms) : std::vector<Bytes>();
      if (certificates.empty())
        return;  // answered, but with nothing usable: the server did not answer properly
      mbedtls_x509_crt crt;
      mbedtls_x509_crt_init(&crt);
      if (mbedtls_x509_crt_parse_der(&crt, certificates[0].data(), certificates[0].size()) == 0) {
        reply.result = eink_ports::Result::ISSUED;
        reply.certificate = certificates[0];
        reply.lifetime = {epoch(crt.valid_from), epoch(crt.valid_to)};
        reply.intermediates = session.intermediates();
      }
      mbedtls_x509_crt_free(&crt);
    }
    // Any other status (a malformed request, a server error) leaves UNREACHABLE: reported as the server failing.
  }
};

}  // namespace eink_est
