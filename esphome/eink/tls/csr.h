// The display's PKCS #10 request, signed with its key. The DER is put together in core/der.h (tested on a computer);
// this takes the public key from mbedTLS and has it sign.
#pragma once

#include <string>

#include "mbedtls/md.h"
#include "mbedtls/pk.h"
#include "mbedtls/sha256.h"

#include "eink/core/base64.h"
#include "eink/core/der.h"
#include "eink/tls/tls.h"

namespace eink_csr {

using Bytes = eink_tls::Bytes;

// The key's public half as a DER SubjectPublicKeyInfo (91 bytes for P-256); empty if it cannot be written.
inline Bytes public_key(mbedtls_pk_context &key) {
  unsigned char buffer[200];
  const int n = mbedtls_pk_write_pubkey_der(&key, buffer, sizeof buffer);  // written at the end of the buffer
  if (n <= 0)
    return {};
  return Bytes(buffer + sizeof buffer - n, buffer + sizeof buffer);
}

// A request for `name`, signed with ECDSA and SHA-256. A non-empty `binding` (the connection's RFC 9266 value) goes in
// as the challenge password, in base64, which is how the server reads it. Empty if it could not be signed.
inline Bytes request(mbedtls_pk_context &key, const std::string &name, const Bytes &binding) {
  const Bytes spki = public_key(key);
  if (spki.empty())
    return {};
  const Bytes info = eink_der::request_info(name, spki, binding.empty() ? "" : eink_base64::encode(binding));
  unsigned char hash[32];
  if (mbedtls_sha256(info.data(), info.size(), hash, 0) != 0)
    return {};
  unsigned char signature[MBEDTLS_PK_SIGNATURE_MAX_SIZE];
  size_t length = 0;
  if (mbedtls_pk_sign(&key, MBEDTLS_MD_SHA256, hash, sizeof hash, signature, sizeof signature, &length,
                      eink_tls::Random::generate, nullptr) != 0)
    return {};
  return eink_der::request(info, Bytes(signature, signature + length));
}

}  // namespace eink_csr
