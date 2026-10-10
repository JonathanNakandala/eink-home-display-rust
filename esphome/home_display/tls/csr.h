// The display's PKCS #10 request, signed with its key. The DER is put together in core/der.h (tested on a computer);
// this takes the public key from mbedTLS and has it sign.
#pragma once

#include <string>

#include "mbedtls/md.h"
#include "mbedtls/pk.h"
#include "mbedtls/sha256.h"

#include "home_display/core/base64.h"
#include "home_display/core/der.h"
#include "home_display/tls/tls.h"

namespace home_display_csr {

using Bytes = home_display_tls::Bytes;

// The key's public half as a DER SubjectPublicKeyInfo (91 bytes for P-256); empty if it cannot be written.
inline Bytes public_key(mbedtls_pk_context &key) {
  unsigned char buffer[200];
  const int n = mbedtls_pk_write_pubkey_der(&key, buffer, sizeof buffer);  // written at the end of the buffer
  if (n <= 0)
    return {};
  return Bytes(buffer + sizeof buffer - n, buffer + sizeof buffer);
}

// A request for `name`, signed with ECDSA and SHA-256. A non-empty `binding` (the connection's RFC 9266 value) goes in
// as the challenge password, in base64, which is how the server reads it. A `profile` (core/profile.h, already DER)
// goes in as an extension. Empty if it could not be signed.
inline Bytes request(mbedtls_pk_context &key, const std::string &name, const Bytes &binding,
                     const Bytes &profile = {}) {
  const Bytes spki = public_key(key);
  if (spki.empty())
    return {};
  const Bytes info =
      home_display_der::request_info(name, spki, binding.empty() ? "" : home_display_base64::encode(binding), profile);
  unsigned char hash[32];
  if (mbedtls_sha256(info.data(), info.size(), hash, 0) != 0)
    return {};
  unsigned char signature[MBEDTLS_PK_SIGNATURE_MAX_SIZE];
  size_t length = 0;
  if (mbedtls_pk_sign(&key, MBEDTLS_MD_SHA256, hash, sizeof hash, signature, sizeof signature, &length,
                      home_display_tls::Random::generate, nullptr) != 0)
    return {};
  return home_display_der::request(info, Bytes(signature, signature + length));
}

}  // namespace home_display_csr
