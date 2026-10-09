// Whether a certificate was issued under the root: the check that keeps a certificate from being kept when it is not
// the authority's. mbedTLS does the verifying.
#pragma once

#include <vector>

#include "mbedtls/x509_crt.h"

#include "eink_ports.h"

namespace eink_verifier {

using Bytes = eink_ports::Bytes;

class MbedVerifier : public eink_ports::Verifier {
 public:
  // The certificate, then the intermediates it may need, then the root as the only anchor. The dates are checked
  // against the clock now, which is why nothing is tried until the clock is usable.
  bool chains_to(const Bytes &certificate, const std::vector<Bytes> &intermediates, const Bytes &root) override {
    mbedtls_x509_crt chain, anchor;
    mbedtls_x509_crt_init(&chain);
    mbedtls_x509_crt_init(&anchor);
    bool ok = mbedtls_x509_crt_parse_der(&chain, certificate.data(), certificate.size()) == 0 &&
              mbedtls_x509_crt_parse_der(&anchor, root.data(), root.size()) == 0;
    for (size_t i = 0; ok && i < intermediates.size(); i++)
      ok = mbedtls_x509_crt_parse_der(&chain, intermediates[i].data(), intermediates[i].size()) == 0;
    if (ok) {
      uint32_t flags = 0;
      ok = mbedtls_x509_crt_verify(&chain, &anchor, nullptr, nullptr, &flags, nullptr, nullptr) == 0;
    }
    mbedtls_x509_crt_free(&chain);
    mbedtls_x509_crt_free(&anchor);
    return ok;
  }
};

}  // namespace eink_verifier
