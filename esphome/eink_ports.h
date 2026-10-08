// What the joining logic needs from the outside world, as interfaces: the clock, the key and certificate store, the
// network, and the check that a certificate was signed by a root. The chip implements them with mbedTLS, flash and
// the radio; the tests implement them with fakes, so the whole of joining (ask, wait, renew, recover) runs on a
// computer.
//
// Pure declarations, with nothing from ESPHome or ESP-IDF.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include "eink_pairing.h"

namespace eink_ports {

using Bytes = std::vector<uint8_t>;

// The dates a certificate is valid between, seconds since 1970.
struct Lifetime {
  int64_t not_before = 0;
  int64_t not_after = 0;
};

class Clock {
 public:
  virtual ~Clock() = default;
  virtual int64_t now() = 0;
  // Whether it can be believed enough to check a certificate against (eink_clock.h).
  virtual bool usable() = 0;
};

// The display's own key and what it holds of the authority, in flash. Nothing here is ever removed by an error: only
// the owner removes pairing (a long press, or a flash that erases). Writing is all or nothing, so that a power cut
// leaves the old value (two slots, switched when the new one is complete).
class Identity {
 public:
  virtual ~Identity() = default;

  // The owner's root built into the firmware (`server_root`), or empty. When there is one, nothing is fetched.
  virtual Bytes compiled_root() = 0;
  // The root kept after a certificate was issued under it, or empty.
  virtual Bytes stored_root() = 0;
  virtual bool save_root(const Bytes &der) = 0;

  virtual bool has_key() = 0;
  virtual bool make_key() = 0;  // once; never replaced
  virtual Bytes spki() = 0;     // DER SubjectPublicKeyInfo
  // A PKCS #10 request for `name` signed with the key. `binding` is the connection's RFC 9266 channel binding, or
  // empty.
  virtual Bytes csr(const std::string &name, const Bytes &binding) = 0;

  virtual bool has_certificate() = 0;
  virtual Lifetime lifetime() = 0;  // of the certificate held
  virtual bool save_certificate(const Bytes &der, const Lifetime &lifetime) = 0;

  // What the server last said to this display's request, kept across sleeps.
  virtual eink_pairing::Answer last_answer() = 0;
  virtual void set_last_answer(eink_pairing::Answer answer) = 0;
};

enum class Fetch : uint8_t {
  OK,           // got a root
  UNREACHABLE,  // no connection, or no answer
  BAD,          // answered, but with no usable self-signed root
};

struct RootReply {
  Fetch result = Fetch::UNREACHABLE;
  Bytes root;
};

enum class Result : uint8_t {
  ISSUED,       // 200: a certificate
  PENDING,      // 202: not approved yet
  REFUSED,      // 403
  UNREACHABLE,  // no connection, or no answer
  TLS_REFUSED,  // the connection was refused over a certificate (the server's, or ours)
};

struct Reply {
  Result result = Result::UNREACHABLE;
  Bytes certificate;           // when ISSUED
  Lifetime lifetime;           // of that certificate
  uint32_t retry_after_s = 0;  // when PENDING
};

// EST (RFC 7030) as the display speaks it. Each call is a new connection, so a channel binding is made again.
class Est {
 public:
  virtual ~Est() = default;
  // GET cacerts over a connection that verifies nothing: the root is confirmed later, by the code.
  virtual RootReply fetch_root() = 0;
  // POST simpleenroll without showing a certificate, verifying the server against `root`.
  virtual Reply enroll(const std::string &name, Identity &identity, const Bytes &root) = 0;
  // POST simplereenroll showing the current certificate.
  virtual Reply renew(const std::string &name, Identity &identity, const Bytes &root) = 0;
};

class Verifier {
 public:
  virtual ~Verifier() = default;
  // Whether `certificate` was issued under `root` (a path from the one to the other).
  virtual bool chains_to(const Bytes &certificate, const Bytes &root) = 0;
};

}  // namespace eink_ports
