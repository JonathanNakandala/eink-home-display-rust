// A spike: everything the display has to do with mbedTLS to join and use the server, run against the real server on a
// computer, to find out what the library can and cannot do before any of it is written for the chip.
//
//   spike_tls <port> <displayctl> <admin socket> <name>
//
// against `cargo run --example est_fixture -- <directory> <port> require-binding` (see run_spike.sh). Each step prints
// PASS or FAIL, and the exit status is the number that failed.
#include <arpa/inet.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include "mbedtls/base64.h"
#include "mbedtls/build_info.h"
#include "mbedtls/ctr_drbg.h"
#include "mbedtls/entropy.h"
#include "mbedtls/error.h"
#include "mbedtls/net_sockets.h"
#include "mbedtls/pk.h"
#include "mbedtls/ssl.h"
#include "mbedtls/x509_crt.h"

#include "eink_pairing.h"

using Bytes = std::vector<uint8_t>;

static int failures = 0;
static void report(bool ok, const std::string &what) {
  std::printf("%s  %s\n", ok ? "PASS" : "FAIL", what.c_str());
  if (!ok)
    failures++;
}

static std::string error_text(int code) {
  char buffer[128];
  mbedtls_strerror(code, buffer, sizeof buffer);
  return std::string(buffer) + " (-0x" + [&] {
    char hex[16];
    std::snprintf(hex, sizeof hex, "%04x", -code);
    return std::string(hex);
  }() + ")";
}

// ---- DER, just enough to take a certs-only CMS message apart
// ---------------------------------------------------------

struct Tlv {
  uint8_t tag = 0;
  const uint8_t *value = nullptr;
  size_t length = 0;
  size_t total = 0;  // header plus value
  bool ok = false;
};

static Tlv read_tlv(const uint8_t *p, size_t available) {
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
  if (available < header + length)
    return t;
  t.value = p + header;
  t.length = length;
  t.total = header + length;
  t.ok = true;
  return t;
}

// Appends a TLV to `out`.
static void put_tlv(Bytes &out, uint8_t tag, const Bytes &value) {
  out.push_back(tag);
  if (value.size() < 128) {
    out.push_back(static_cast<uint8_t>(value.size()));
  } else if (value.size() < 256) {
    out.push_back(0x81);
    out.push_back(static_cast<uint8_t>(value.size()));
  } else {
    out.push_back(0x82);
    out.push_back(static_cast<uint8_t>(value.size() >> 8));
    out.push_back(static_cast<uint8_t>(value.size()));
  }
  out.insert(out.end(), value.begin(), value.end());
}

static Bytes tlv(uint8_t tag, const Bytes &value) {
  Bytes out;
  put_tlv(out, tag, value);
  return out;
}

static Bytes concat(std::initializer_list<Bytes> parts) {
  Bytes out;
  for (const Bytes &p : parts)
    out.insert(out.end(), p.begin(), p.end());
  return out;
}

// The certificates in a certs-only CMS SignedData: ContentInfo { oid, [0] { SignedData { version, digestAlgorithms,
// encapContentInfo, [0] IMPLICIT certificates, ... } } }.
static std::vector<Bytes> certificates_in(const Bytes &cms) {
  std::vector<Bytes> out;
  Tlv content_info = read_tlv(cms.data(), cms.size());
  if (!content_info.ok || content_info.tag != 0x30)
    return out;
  const uint8_t *p = content_info.value;
  size_t left = content_info.length;
  Tlv oid = read_tlv(p, left);  // contentType
  if (!oid.ok)
    return out;
  p += oid.total;
  left -= oid.total;
  Tlv explicit0 = read_tlv(p, left);  // [0] EXPLICIT
  if (!explicit0.ok || explicit0.tag != 0xA0)
    return out;
  Tlv signed_data = read_tlv(explicit0.value, explicit0.length);
  if (!signed_data.ok || signed_data.tag != 0x30)
    return out;
  p = signed_data.value;
  left = signed_data.length;
  while (left > 0) {
    Tlv item = read_tlv(p, left);
    if (!item.ok)
      return out;
    if (item.tag == 0xA0) {  // [0] IMPLICIT certificates
      const uint8_t *c = item.value;
      size_t cleft = item.length;
      while (cleft > 0) {
        Tlv cert = read_tlv(c, cleft);
        if (!cert.ok)
          return out;
        out.emplace_back(c, c + cert.total);
        c += cert.total;
        cleft -= cert.total;
      }
    }
    p += item.total;
    left -= item.total;
  }
  return out;
}

// ---- base64
// ----------------------------------------------------------------------------------------------------------

static std::string base64(const Bytes &data) {
  size_t length = 0;
  mbedtls_base64_encode(nullptr, 0, &length, data.data(), data.size());
  std::string out(length, '\0');
  mbedtls_base64_encode(reinterpret_cast<unsigned char *>(&out[0]), out.size(), &length, data.data(), data.size());
  out.resize(length);
  return out;
}

static Bytes unbase64(const std::string &text) {
  std::string clean;
  for (char c : text)
    if (c != '\r' && c != '\n' && c != ' ')
      clean += c;
  size_t length = 0;
  mbedtls_base64_decode(nullptr, 0, &length, reinterpret_cast<const unsigned char *>(clean.data()), clean.size());
  Bytes out(length);
  if (mbedtls_base64_decode(out.data(), out.size(), &length, reinterpret_cast<const unsigned char *>(clean.data()),
                            clean.size()) != 0)
    return {};
  out.resize(length);
  return out;
}

static std::string hex(const Bytes &data) {
  std::string out;
  char two[3];
  for (uint8_t b : data) {
    std::snprintf(two, sizeof two, "%02x", b);
    out += two;
  }
  return out;
}

// ---- a TLS 1.3 connection
// ----------------------------------------------------------------------------------------------

static mbedtls_entropy_context entropy;
static mbedtls_ctr_drbg_context drbg;

struct Options {
  const mbedtls_x509_crt *trust = nullptr;  // null: verify nothing
  std::string hostname;                     // what the certificate must name
  const mbedtls_x509_crt *own = nullptr;
  mbedtls_pk_context *own_key = nullptr;
  mbedtls_ssl_session *resume = nullptr;
  mbedtls_ssl_session *keep = nullptr;  // where to put the newest session ticket the server sends
};

struct Connection {
  mbedtls_net_context net;
  mbedtls_ssl_context ssl;
  mbedtls_ssl_config conf;
  int handshake = -1;
  uint32_t verify_flags = 0;
  int tickets = 0;  // new-session tickets seen while reading
  mbedtls_ssl_session *keep = nullptr;

  Connection() {
    mbedtls_net_init(&net);
    mbedtls_ssl_init(&ssl);
    mbedtls_ssl_config_init(&conf);
  }
  ~Connection() {
    mbedtls_ssl_free(&ssl);
    mbedtls_ssl_config_free(&conf);
    mbedtls_net_free(&net);
  }
  Connection(const Connection &) = delete;

  // 0 on success; a library error code on failure (also kept in `handshake`).
  int open(uint16_t port, const Options &o) {
    char port_text[8];
    std::snprintf(port_text, sizeof port_text, "%u", port);
    int r = mbedtls_net_connect(&net, "127.0.0.1", port_text, MBEDTLS_NET_PROTO_TCP);
    if (r != 0)
      return handshake = r;
    r = mbedtls_ssl_config_defaults(&conf, MBEDTLS_SSL_IS_CLIENT, MBEDTLS_SSL_TRANSPORT_STREAM,
                                    MBEDTLS_SSL_PRESET_DEFAULT);
    if (r != 0)
      return handshake = r;
    // TLS 1.3 and nothing older: the server offers nothing else.
    mbedtls_ssl_conf_min_tls_version(&conf, MBEDTLS_SSL_VERSION_TLS1_3);
    mbedtls_ssl_conf_max_tls_version(&conf, MBEDTLS_SSL_VERSION_TLS1_3);
    mbedtls_ssl_conf_rng(&conf, mbedtls_ctr_drbg_random, &drbg);
    if (o.trust != nullptr) {
      mbedtls_ssl_conf_authmode(&conf, MBEDTLS_SSL_VERIFY_REQUIRED);
      mbedtls_ssl_conf_ca_chain(&conf, const_cast<mbedtls_x509_crt *>(o.trust), nullptr);
    } else {
      mbedtls_ssl_conf_authmode(&conf, MBEDTLS_SSL_VERIFY_NONE);
    }
    // In TLS 1.3 a ticket arrives after the handshake, and the library keeps it only if the application asks to be told
    // (mbedtls_ssl_read then returns MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET, and the session is taken then).
    if (o.keep != nullptr) {
      keep = o.keep;
      mbedtls_ssl_conf_tls13_enable_signal_new_session_tickets(&conf,
                                                               MBEDTLS_SSL_TLS1_3_SIGNAL_NEW_SESSION_TICKETS_ENABLED);
    }
    if (o.own != nullptr)
      mbedtls_ssl_conf_own_cert(&conf, const_cast<mbedtls_x509_crt *>(o.own), o.own_key);
    r = mbedtls_ssl_setup(&ssl, &conf);
    if (r != 0)
      return handshake = r;
    // The name the certificate must have, whatever address was connected to.
    if (!o.hostname.empty() && (r = mbedtls_ssl_set_hostname(&ssl, o.hostname.c_str())) != 0)
      return handshake = r;
    if (o.resume != nullptr && (r = mbedtls_ssl_set_session(&ssl, o.resume)) != 0)
      return handshake = r;
    mbedtls_ssl_set_bio(&ssl, &net, mbedtls_net_send, mbedtls_net_recv, nullptr);
    while ((r = mbedtls_ssl_handshake(&ssl)) == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE) {
    }
    verify_flags = mbedtls_ssl_get_verify_result(&ssl);
    return handshake = r;
  }

  // One request and its whole answer, over this connection, which the server then closes.
  std::string request(const std::string &method, const std::string &path, const std::string &content_type,
                      const std::string &body, int &status, std::string &headers) {
    std::string head = method + " " + path + " HTTP/1.1\r\nHost: eink-home-display.internal\r\nConnection: close\r\n";
    if (!content_type.empty())
      head += "Content-Type: " + content_type + "\r\n";
    head += "Content-Length: " + std::to_string(body.size()) + "\r\n\r\n";
    const std::string all = head + body;
    size_t sent = 0;
    while (sent < all.size()) {
      int r = mbedtls_ssl_write(&ssl, reinterpret_cast<const unsigned char *>(all.data()) + sent, all.size() - sent);
      if (r == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE)
        continue;
      if (r < 0) {
        status = -1;
        return "";
      }
      sent += r;
    }
    std::string raw;
    unsigned char buffer[2048];
    for (;;) {
      int r = mbedtls_ssl_read(&ssl, buffer, sizeof buffer);
      if (r == MBEDTLS_ERR_SSL_WANT_READ || r == MBEDTLS_ERR_SSL_WANT_WRITE)
        continue;
      // TLS 1.3: a ticket for a later session arrived. It is kept by the library; reading goes on.
      if (r == MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET) {
        tickets++;
        if (keep != nullptr) {
          mbedtls_ssl_session_free(keep);
          mbedtls_ssl_session_init(keep);
          mbedtls_ssl_get_session(&ssl, keep);
        }
        continue;
      }
      if (r <= 0)
        break;  // closed (cleanly or not): what arrived is the answer
      raw.append(reinterpret_cast<char *>(buffer), r);
    }
    const size_t split = raw.find("\r\n\r\n");
    if (split == std::string::npos) {
      status = -1;
      return "";
    }
    headers = raw.substr(0, split);
    status = std::atoi(headers.c_str() + headers.find(' ') + 1);
    std::string payload = raw.substr(split + 4);
    if (headers.find("chunked") != std::string::npos || headers.find("Chunked") != std::string::npos) {
      std::string joined;
      size_t at = 0;
      while (at < payload.size()) {
        const size_t eol = payload.find("\r\n", at);
        if (eol == std::string::npos)
          break;
        const size_t size = std::strtoul(payload.substr(at, eol - at).c_str(), nullptr, 16);
        if (size == 0)
          break;
        joined += payload.substr(eol + 2, size);
        at = eol + 2 + size + 2;
      }
      payload = joined;
    }
    return payload;
  }
};

static std::string header_value(const std::string &headers, const std::string &name) {
  std::string lower = headers;
  for (char &c : lower)
    c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  const size_t at = lower.find("\r\n" + name + ":");
  if (at == std::string::npos)
    return "";
  const size_t start = at + 2 + name.size() + 1;
  const size_t end = headers.find("\r\n", start);
  std::string v = headers.substr(start, end == std::string::npos ? std::string::npos : end - start);
  while (!v.empty() && v[0] == ' ')
    v.erase(0, 1);
  return v;
}

// ---- a PKCS #10 request, with the channel binding as its challenge password
// ----------------------------------------------

static const Bytes OID_COMMON_NAME = {0x06, 0x03, 0x55, 0x04, 0x03};
static const Bytes OID_CHALLENGE_PASSWORD = {0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x07};
static const Bytes ECDSA_WITH_SHA256 = {0x30, 0x0A, 0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02};

static Bytes spki_of(mbedtls_pk_context &key) {
  unsigned char buffer[200];
  const int n = mbedtls_pk_write_pubkey_der(&key, buffer, sizeof buffer);  // written at the end of the buffer
  if (n <= 0)
    return {};
  return Bytes(buffer + sizeof buffer - n, buffer + sizeof buffer);
}

// mbedTLS can write a request, but not with a challenge-password attribute (its writer takes extensions only), so the
// request is put together by hand and signed with the library.
static Bytes csr(mbedtls_pk_context &key, const std::string &name, const std::string &challenge) {
  const Bytes version = {0x02, 0x01, 0x00};
  const Bytes subject =
      tlv(0x30, tlv(0x31, tlv(0x30, concat({OID_COMMON_NAME, tlv(0x0C, Bytes(name.begin(), name.end()))}))));
  Bytes attributes;
  if (!challenge.empty())
    attributes =
        tlv(0x30, concat({OID_CHALLENGE_PASSWORD, tlv(0x31, tlv(0x0C, Bytes(challenge.begin(), challenge.end())))}));
  const Bytes info = tlv(0x30, concat({version, subject, spki_of(key), tlv(0xA0, attributes)}));

  unsigned char hash[32];
  mbedtls_sha256(info.data(), info.size(), hash, 0);
  unsigned char signature[MBEDTLS_PK_SIGNATURE_MAX_SIZE];
  size_t signature_length = 0;
  if (mbedtls_pk_sign(&key, MBEDTLS_MD_SHA256, hash, sizeof hash, signature, sizeof signature, &signature_length,
                      mbedtls_ctr_drbg_random, &drbg) != 0)
    return {};
  Bytes bits = {0x00};  // no unused bits
  bits.insert(bits.end(), signature, signature + signature_length);
  return tlv(0x30, concat({info, ECDSA_WITH_SHA256, tlv(0x03, bits)}));
}

// ---- the steps
// -------------------------------------------------------------------------------------------------------

static const char *SERVER_NAME = "eink-home-display.internal";
static const char *ESTD = "/.well-known/est/";

static Bytes channel_binding(Connection &c) {
  Bytes out(32);
  const char *label = "EXPORTER-Channel-Binding";
  if (mbedtls_ssl_export_keying_material(&c.ssl, out.data(), out.size(), label, std::strlen(label), nullptr, 0, 0) != 0)
    return {};
  return out;
}

int main(int argc, char **argv) {
  if (argc < 5) {
    std::fprintf(stderr, "usage: spike_tls <port> <displayctl> <admin socket> <name>\n");
    return 2;
  }
  const uint16_t port = static_cast<uint16_t>(std::atoi(argv[1]));
  const std::string displayctl = argv[2], admin = argv[3], name = argv[4];
  std::printf("mbedTLS %s\n", MBEDTLS_VERSION_STRING);

  mbedtls_entropy_init(&entropy);
  mbedtls_ctr_drbg_init(&drbg);
  mbedtls_ctr_drbg_seed(&drbg, mbedtls_entropy_func, &entropy, nullptr, 0);
  int status = 0;
  std::string headers;

  // 1. First contact: fetch the authority over a connection that verifies nothing, and find the root in it.
  Bytes root_der;
  {
    Connection c;
    report(c.open(port, Options{}) == 0, "an unverified TLS 1.3 connection to the server");
    report(std::string(mbedtls_ssl_get_version(&c.ssl)) == "TLSv1.3", "it is TLS 1.3");
    const std::string body = c.request("GET", std::string(ESTD) + "cacerts", "", "", status, headers);
    report(status == 200, "GET cacerts answers 200 (got " + std::to_string(status) + ")");
    const std::vector<Bytes> certificates = certificates_in(unbase64(body));
    report(certificates.size() == 2,
           "the CMS holds two certificates (root and intermediate), found " + std::to_string(certificates.size()));
    for (const Bytes &der : certificates) {
      mbedtls_x509_crt crt;
      mbedtls_x509_crt_init(&crt);
      if (mbedtls_x509_crt_parse_der(&crt, der.data(), der.size()) == 0 && crt.subject_raw.len == crt.issuer_raw.len &&
          std::memcmp(crt.subject_raw.p, crt.issuer_raw.p, crt.subject_raw.len) == 0) {
        uint32_t flags = 0;
        const bool signed_by_itself =
            mbedtls_x509_crt_verify(&crt, &crt, nullptr, nullptr, &flags, nullptr, nullptr) == 0;
        report(signed_by_itself, "the self-signed certificate verifies against itself: it is the root");
        if (signed_by_itself)
          root_der = der;
      }
      mbedtls_x509_crt_free(&crt);
    }
    report(!root_der.empty(), "a root was picked out of the CMS");
  }
  if (root_der.empty())
    return failures;
  mbedtls_x509_crt root;
  mbedtls_x509_crt_init(&root);
  mbedtls_x509_crt_parse_der(&root, root_der.data(), root_der.size());

  // 2. The server is checked against the root and the fixed name, though the address connected to is 127.0.0.1.
  {
    Connection c;
    Options o;
    o.trust = &root;
    o.hostname = SERVER_NAME;
    const int r = c.open(port, o);
    report(r == 0, "the server verifies against the pinned root and the name " + std::string(SERVER_NAME) +
                       (r == 0 ? "" : " -> " + error_text(r)));
    if (r == 0) {
      const Bytes binding = channel_binding(c);
      report(binding.size() == 32, "the RFC 9266 channel binding can be exported (mbedtls_ssl_export_keying_material)");
      std::printf("      binding %s\n", hex(binding).c_str());
    }
  }
  {
    Connection c;
    Options o;
    o.trust = &root;
    o.hostname = "other.internal";
    const int r = c.open(port, o);
    report(r != 0 && (c.verify_flags & MBEDTLS_X509_BADCERT_CN_MISMATCH) != 0,
           "a certificate for another name is refused (" + error_text(r) + ")");
  }
  {
    // A root of someone else's: the server must not be believed.
    mbedtls_x509_crt other;
    mbedtls_x509_crt_init(&other);
    mbedtls_pk_context other_key;
    mbedtls_pk_init(&other_key);
    mbedtls_pk_setup(&other_key, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY));
    mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(other_key), mbedtls_ctr_drbg_random, &drbg);
    // (an empty trust list: nothing is trusted, so nothing verifies)
    Connection c;
    Options o;
    o.trust = &other;
    o.hostname = SERVER_NAME;
    const int r = c.open(port, o);
    report(r != 0, "a server that does not chain to the pinned root is refused (" + error_text(r) + ")");
    mbedtls_pk_free(&other_key);
    mbedtls_x509_crt_free(&other);
  }

  // 3. The display's key, and a request for a certificate that carries the channel binding of the connection it is sent
  // on.
  mbedtls_pk_context key;
  mbedtls_pk_init(&key);
  mbedtls_pk_setup(&key, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY));
  report(mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(key), mbedtls_ctr_drbg_random, &drbg) == 0,
         "an ECDSA P-256 key is made");
  const Bytes spki = spki_of(key);
  report(spki.size() == 91, "its SubjectPublicKeyInfo is 91 bytes, like the documentation's example (" +
                                std::to_string(spki.size()) + ")");
  const std::string code = eink_pairing::code(root_der, name, spki);
  std::printf("      the pairing code on the panel: %s\n", code.c_str());

  std::vector<Bytes> peer_chain;  // from the last connection enroll() made
  auto enroll = [&](const char *path, const mbedtls_x509_crt *own, std::string &answer, std::string &answer_headers,
                    bool wrong_binding = false) {
    Connection c;
    Options o;
    o.trust = &root;
    o.hostname = SERVER_NAME;
    o.own = own;
    o.own_key = own != nullptr ? &key : nullptr;
    if (c.open(port, o) != 0)
      return -2;
    Bytes binding = channel_binding(c);
    if (wrong_binding && !binding.empty())
      binding[0] ^= 0xFF;
    const Bytes request = csr(key, name, base64(binding));
    int code_returned = 0;
    answer = c.request("POST", std::string(ESTD) + path, "application/pkcs10", base64(request), code_returned,
                       answer_headers);
    // What the server presented beyond its own certificate: its intermediate(s), which an issued certificate needs.
    peer_chain.clear();
    const mbedtls_x509_crt *peer = mbedtls_ssl_get_peer_cert(&c.ssl);
    for (const mbedtls_x509_crt *p = peer != nullptr ? peer->next : nullptr; p != nullptr; p = p->next)
      peer_chain.emplace_back(p->raw.p, p->raw.p + p->raw.len);
    return code_returned;
  };

  std::string body;
  const int wrong = enroll("simpleenroll", nullptr, body, headers, true);
  report(wrong >= 400 && wrong < 500,
         "a request whose binding is not this connection's is refused (" + std::to_string(wrong) + ")");

  const int asked = enroll("simpleenroll", nullptr, body, headers);
  report(asked == 202, "the first request is answered 202, not approved yet (" + std::to_string(asked) + ")");
  std::printf("      Retry-After: %s\n", header_value(headers, "retry-after").c_str());

  // 4. The owner types the code in at the server: if the display's code is right, the server approves.
  const std::string approve = displayctl + " --socket " + admin + " approve " + name + " " + code + " > /dev/null 2>&1";
  report(std::system(approve.c_str()) == 0, "the server approves the code the display worked out");

  const int granted = enroll("simpleenroll", nullptr, body, headers);
  report(granted == 200, "after approval the next request is answered 200 (" + std::to_string(granted) + ")");
  const std::vector<Bytes> issued = certificates_in(unbase64(body));
  report(issued.size() >= 1, "the answer holds the display's certificate");
  if (issued.empty())
    return failures;
  mbedtls_x509_crt mine;
  mbedtls_x509_crt_init(&mine);
  report(mbedtls_x509_crt_parse_der(&mine, issued[0].data(), issued[0].size()) == 0, "the certificate parses");
  std::printf("      certificates in the answer: %zu; the server presented %zu beyond its own\n", issued.size(),
              peer_chain.size());
  {
    // An enrolment answer carries the display's certificate alone, so on its own it cannot be checked against the root:
    // the intermediate that signed it is what the server presented in the TLS handshake, which is where the display
    // takes it from (the connection was verified against the root, so what it presents is trusted to that extent).
    mbedtls_x509_crt alone;
    mbedtls_x509_crt_init(&alone);
    mbedtls_x509_crt_parse_der(&alone, issued[0].data(), issued[0].size());
    uint32_t alone_flags = 0;
    report(mbedtls_x509_crt_verify(&alone, &root, nullptr, nullptr, &alone_flags, nullptr, nullptr) != 0,
           "the certificate alone does NOT chain to the root (its issuer is the intermediate)");
    mbedtls_x509_crt_free(&alone);

    mbedtls_x509_crt chain;
    mbedtls_x509_crt_init(&chain);
    for (const Bytes &der : issued)
      mbedtls_x509_crt_parse_der(&chain, der.data(), der.size());
    for (const Bytes &der : peer_chain)
      mbedtls_x509_crt_parse_der(&chain, der.data(), der.size());
    uint32_t flags = 0;
    const int r = mbedtls_x509_crt_verify(&chain, &root, nullptr, nullptr, &flags, nullptr, nullptr);
    report(r == 0, "with the intermediate the server presented, it chains to the root" +
                       (r == 0 ? "" : " (" + error_text(r) + ", flags " + std::to_string(flags) + ")"));
    mbedtls_x509_crt_free(&chain);
  }

  // 5. Using it: the server names the display by its certificate, and a second connection can resume the first.
  mbedtls_ssl_session saved;
  mbedtls_ssl_session_init(&saved);
  {
    Connection c;
    Options o;
    o.trust = &root;
    o.hostname = SERVER_NAME;
    o.own = &mine;
    o.own_key = &key;
    o.keep = &saved;
    const int r = c.open(port, o);
    report(r == 0, "a connection that shows the certificate is accepted" + (r == 0 ? "" : " -> " + error_text(r)));
    const std::string who = c.request("GET", "/who", "", "", status, headers);
    report(status == 200 && who == name, "the server names the display by its certificate: \"" + who + "\"");
    std::printf("      tickets received: %d\n", c.tickets);
    report(c.tickets > 0, "the server sent a session ticket");
  }
  {
    Connection c;
    Options o;
    o.trust = &root;
    o.hostname = SERVER_NAME;
    o.own = &mine;
    o.own_key = &key;
    o.resume = &saved;
    const int r = c.open(port, o);
    report(r == 0, "a connection that offers the saved session is accepted" + (r == 0 ? "" : " -> " + error_text(r)));
    const std::string who = c.request("GET", "/who", "", "", status, headers);
    report(status == 200 && who == name, "and is still known as the display: \"" + who + "\"");
  }
  {
    Connection c;
    Options o;
    o.trust = &root;
    o.hostname = SERVER_NAME;
    o.own = &mine;
    o.own_key = &key;
    c.open(port, o);
    const std::string counts = c.request("GET", "/handshakes", "", "", status, headers);
    std::printf("      the server counts: %s\n", counts.c_str());
    report(counts.find("resumed=0") == std::string::npos && counts.find("resumed=") != std::string::npos,
           "the server counted a resumed handshake");
  }

  // 6. Renewal needs no owner: it shows the certificate and is answered with a new one.
  const int renewed = enroll("simplereenroll", &mine, body, headers);
  report(renewed == 200, "renewing with the certificate shown is answered 200 (" + std::to_string(renewed) + ")");

  mbedtls_x509_crt_free(&mine);
  mbedtls_x509_crt_free(&root);
  mbedtls_pk_free(&key);
  mbedtls_ssl_session_free(&saved);
  std::printf("%d failed\n", failures);
  return failures;
}
