// The little of ASN.1 DER the display needs: taking a certs-only CMS message apart, and putting a PKCS #10 request
// together. mbedTLS has no CMS, and its request writer cannot add a challenge password, so these are done here.
//
// The whole is also run against a real server in tests/tls/.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace home_display_der {

using Bytes = std::vector<uint8_t>;

// One tag, length and value, read from the start of a buffer.
struct Tlv {
  uint8_t tag = 0;
  const uint8_t *value = nullptr;
  size_t length = 0;
  size_t total = 0;  // the header and the value
  bool ok = false;
};

// Reads the element at `p`. Not ok if it is cut short, its length is wider than four bytes, or the length claims more
// than is there. Only definite lengths exist in DER.
inline Tlv read(const uint8_t *p, size_t available) {
  Tlv t;
  if (available < 2)
    return t;
  t.tag = p[0];
  size_t header = 2, length = p[1];
  if (length & 0x80) {
    const size_t count = length & 0x7f;
    if (count == 0 || count > 4 || available < 2 + count)
      return t;
    length = 0;
    for (size_t i = 0; i < count; i++)
      length = (length << 8) | p[2 + i];
    header = 2 + count;
  }
  if (length > available - header)
    return t;
  t.value = p + header;
  t.length = length;
  t.total = header + length;
  t.ok = true;
  return t;
}

inline Bytes tlv(uint8_t tag, const Bytes &value) {
  Bytes out;
  out.push_back(tag);
  const size_t n = value.size();
  if (n < 0x80) {
    out.push_back(static_cast<uint8_t>(n));
  } else if (n < 0x100) {
    out.push_back(0x81);
    out.push_back(static_cast<uint8_t>(n));
  } else if (n < 0x10000) {
    out.push_back(0x82);
    out.push_back(static_cast<uint8_t>(n >> 8));
    out.push_back(static_cast<uint8_t>(n));
  } else {
    out.push_back(0x83);
    out.push_back(static_cast<uint8_t>(n >> 16));
    out.push_back(static_cast<uint8_t>(n >> 8));
    out.push_back(static_cast<uint8_t>(n));
  }
  out.insert(out.end(), value.begin(), value.end());
  return out;
}

inline Bytes concat(std::initializer_list<Bytes> parts) {
  Bytes out;
  for (const Bytes &part : parts)
    out.insert(out.end(), part.begin(), part.end());
  return out;
}

// The certificates in a certs-only CMS SignedData (RFC 5652, RFC 8951), each as its own DER. The message is
//   ContentInfo { contentType, [0] { SignedData { version, digestAlgorithms, encapContentInfo,
//                                                  [0] IMPLICIT certificates, ... } } }
// Empty if it is not that shape. Nothing is checked about the certificates themselves.
inline std::vector<Bytes> certificates_in(const Bytes &cms) {
  std::vector<Bytes> out;
  const Tlv content_info = read(cms.data(), cms.size());
  if (!content_info.ok || content_info.tag != 0x30)
    return out;
  const Tlv type = read(content_info.value, content_info.length);  // contentType
  if (!type.ok)
    return out;
  const Tlv wrapped = read(content_info.value + type.total, content_info.length - type.total);  // [0] EXPLICIT
  if (!wrapped.ok || wrapped.tag != 0xA0)
    return out;
  const Tlv signed_data = read(wrapped.value, wrapped.length);
  if (!signed_data.ok || signed_data.tag != 0x30)
    return out;
  const uint8_t *p = signed_data.value;
  size_t left = signed_data.length;
  while (left > 0) {
    const Tlv item = read(p, left);
    if (!item.ok)
      return {};
    if (item.tag == 0xA0) {  // [0] IMPLICIT SET OF certificates
      const uint8_t *c = item.value;
      size_t remaining = item.length;
      while (remaining > 0) {
        const Tlv cert = read(c, remaining);
        if (!cert.ok)
          return {};
        out.emplace_back(c, c + cert.total);
        c += cert.total;
        remaining -= cert.total;
      }
    }
    p += item.total;
    left -= item.total;
  }
  return out;
}

// ---- a PKCS #10 request (RFC 2986) ----------------------------------------------------------------------------------

// id-at-commonName, pkcs-9-challengePassword and ecdsa-with-SHA256, as encoded.
inline const Bytes &oid_common_name() {
  static const Bytes v = {0x06, 0x03, 0x55, 0x04, 0x03};
  return v;
}
inline const Bytes &oid_challenge_password() {
  static const Bytes v = {0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x07};
  return v;
}
inline const Bytes &ecdsa_with_sha256() {
  static const Bytes v = {0x30, 0x0A, 0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02};
  return v;
}

// The part of the request that is signed: version 0, the subject (a single common name), the public key as the
// SubjectPublicKeyInfo DER, and the attributes, which hold the challenge password when there is one (the channel
// binding, as base64 text).
inline Bytes request_info(const std::string &name, const Bytes &spki, const std::string &challenge_password) {
  const Bytes version = {0x02, 0x01, 0x00};
  const Bytes subject =
      tlv(0x30, tlv(0x31, tlv(0x30, concat({oid_common_name(), tlv(0x0C, Bytes(name.begin(), name.end()))}))));
  Bytes attributes;
  if (!challenge_password.empty()) {
    const Bytes value(challenge_password.begin(), challenge_password.end());
    attributes = tlv(0x30, concat({oid_challenge_password(), tlv(0x31, tlv(0x0C, value))}));
  }
  return tlv(0x30, concat({version, subject, spki, tlv(0xA0, attributes)}));
}

// The finished request: the signed part, the algorithm, and the signature (DER of an ECDSA signature) as a BIT STRING.
inline Bytes request(const Bytes &info, const Bytes &der_signature) {
  Bytes bits = {0x00};  // no unused bits
  bits.insert(bits.end(), der_signature.begin(), der_signature.end());
  return tlv(0x30, concat({info, ecdsa_with_sha256(), tlv(0x03, bits)}));
}

}  // namespace home_display_der
