// A TLS session the display keeps between connections, and between wakes, so that the next connection to the server can
// resume it instead of shaking hands in full: no certificate exchange, no signatures, and a third of the bytes.
//
// What is kept is mbedTLS's own serialisation of the session (it holds the server's ticket and the secrets to use it),
// which is secret: it lets whoever holds it resume as this display. So it lives where the key lives, in memory the
// display owns (RTC memory on the chip, which survives deep sleep and is lost with the power), and never in the log or
// on the wire.
//
// Three parts, with nothing from ESPHome, ESP-IDF or mbedTLS, tested on a computer (tests/core/): a fixed-size slot
// laid out to sit in RTC memory, with a magic, a version and a CRC, so memory an earlier firmware or a power cut left
// behind is not believed; a key saying which server and identity a session is for, so it is never offered to another
// server nor kept across a renewed certificate or a changed root; and how long it has been kept, from the wall clock,
// since the clock mbedTLS uses (monotonic) starts again after deep sleep.
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>

#include "eink/core/sha256.h"

namespace eink_session_cache {

using Bytes = std::vector<uint8_t>;

// The most a session may be to be kept. RTC memory is 8 KB for everything that uses it. How big a real session is,
// with a client certificate, is measured by `what_a_session_costs_in_memory_and_in_bytes_on_the_wire`
// (tests/tls/resume_host_test.cpp), which fails if one outgrows half of this.
constexpr size_t CAPACITY = 3072;

constexpr uint32_t MAGIC = 0xE1B70003;
constexpr uint8_t VERSION = 1;

// Which server and which identity a session is for: the first eight bytes of SHA-256 over the address, the port, the
// root and the certificate shown, each with its length. A different server, port, root or certificate is a different
// key.
using Key = std::array<uint8_t, 8>;

inline Key make_key(uint32_t ip, uint16_t port, const Bytes &root, const Bytes &certificate) {
  eink_sha256::Hash hash;
  hash.update("eink-home-display session cache v1");
  hash.update_length(ip);
  hash.update_length(port);
  hash.update_length(root.size());
  hash.update(root);
  hash.update_length(certificate.size());
  hash.update(certificate);
  const eink_sha256::Digest digest = hash.finish();
  Key key;
  for (size_t i = 0; i < key.size(); i++)
    key[i] = digest[i];
  return key;
}

// What sits in memory. Plain data, so it can be placed in RTC memory and read after a sleep.
struct Slot {
  uint32_t magic;
  uint8_t version;
  uint8_t reserved[3];
  Key key;
  int64_t saved_at;  // seconds since 1970, by the wall clock, when the session was saved
  uint32_t length;
  uint32_t crc;  // of everything above and the data
  uint8_t data[CAPACITY];
};

constexpr Slot EMPTY = {};

inline uint32_t crc_of(const Slot &slot) {
  // The header without the crc field, then the data that is used.
  uint32_t crc = 0xFFFFFFFFu;
  const auto feed = [&crc](const uint8_t *p, size_t n) {
    for (size_t i = 0; i < n; i++) {
      crc ^= p[i];
      for (int bit = 0; bit < 8; bit++)
        crc = (crc >> 1) ^ (0xEDB88320u & (0u - (crc & 1u)));
    }
  };
  feed(reinterpret_cast<const uint8_t *>(&slot), offsetof(Slot, crc));
  feed(slot.data, slot.length <= CAPACITY ? slot.length : 0);
  return ~crc;
}

inline void clear(Slot &slot) { std::memset(&slot, 0, sizeof slot); }

// Keeps `session` for `key`, dated `now`. False if it will not fit (and nothing is kept, and anything held before is
// dropped: a session that is too big is not worth the memory, and an old one for the same server would only fail).
inline bool store(Slot &slot, const Key &key, const Bytes &session, int64_t now) {
  clear(slot);
  if (session.empty() || session.size() > CAPACITY)
    return false;
  slot.magic = MAGIC;
  slot.version = VERSION;
  slot.key = key;
  slot.saved_at = now;
  slot.length = static_cast<uint32_t>(session.size());
  std::memcpy(slot.data, session.data(), session.size());
  slot.crc = crc_of(slot);
  return true;
}

// Why a session was not given back.
enum class Why : uint8_t {
  OK,
  EMPTY,        // nothing kept
  DAMAGED,      // not a slot this firmware wrote, or the CRC does not match
  OTHER_KEY,    // kept for another server or identity
  FROM_FUTURE,  // saved at a time later than now: the wall clock went back, so how long it has been is unknown
  TOO_OLD,      // kept longer than `max_age_s`
};

struct Loaded {
  Why why = Why::EMPTY;
  Bytes session;
  int64_t age_s = 0;  // how long it has been kept, by the wall clock
};

// The session for `key`, if there is one worth trying. `now` is the wall clock and must be one that can be believed:
// the caller does not call this otherwise. `max_age_s` is the longest it is worth trying (the server's ticket lifetime
// is the real limit; this is only to avoid offering one that is certainly over).
inline Loaded load(const Slot &slot, const Key &key, int64_t now, int64_t max_age_s) {
  Loaded out;
  if (slot.magic == 0 && slot.length == 0)
    return out;  // EMPTY
  if (slot.magic != MAGIC || slot.version != VERSION || slot.length == 0 || slot.length > CAPACITY ||
      crc_of(slot) != slot.crc) {
    out.why = Why::DAMAGED;
    return out;
  }
  if (slot.key != key) {
    out.why = Why::OTHER_KEY;
    return out;
  }
  if (now < slot.saved_at) {
    out.why = Why::FROM_FUTURE;
    return out;
  }
  out.age_s = now - slot.saved_at;
  if (out.age_s > max_age_s) {
    out.why = Why::TOO_OLD;
    return out;
  }
  out.session.assign(slot.data, slot.data + slot.length);
  out.why = Why::OK;
  return out;
}

// ---- what the connection code sees ----------------------------------------------------------------------------------

// Where a connection finds the session to resume and leaves the one it gets. One per server and identity.
class Store {
 public:
  virtual ~Store() = default;
  // The session to offer, and how many seconds it has been kept; false if there is none worth offering.
  virtual bool load(Bytes &session, int64_t &age_s) = 0;
  // A newer session (a ticket arrived). Replaces what was kept.
  virtual void save(const Bytes &session) = 0;
  // What was kept did not work, or the identity changed: forget it.
  virtual void forget() = 0;
};

// Picks the Store for a server and an identity. The wake script learns which they are only once the display has joined,
// so it is given this and asks for the store then.
class Holder {
 public:
  virtual ~Holder() = default;
  virtual Store *select(const Key &key) = 0;
};

// A Store over a Slot, a key and the wall clock. `now` returns seconds since 1970, or a negative number when the clock
// cannot be believed (not set by SNTP yet), in which case nothing is kept and nothing is offered.
template <typename Now> class SlotStore : public Store, public Holder {
 public:
  SlotStore(Slot &slot, const Key &key, Now now, int64_t max_age_s = 6 * 3600)
      : slot_(slot), key_(key), now_(now), max_age_s_(max_age_s) {}

  bool load(Bytes &session, int64_t &age_s) override {
    const int64_t now = now_();
    if (now < 0)
      return false;
    Loaded found = eink_session_cache::load(slot_, key_, now, max_age_s_);
    if (found.why == Why::OK) {
      session = std::move(found.session);
      age_s = found.age_s;
      return true;
    }
    // Anything that is not simply "nothing here" is dropped, so a damaged or stale slot is not read again every wake.
    if (found.why != Why::EMPTY)
      clear(slot_);
    return false;
  }

  void save(const Bytes &session) override {
    const int64_t now = now_();
    if (now < 0 || !store(slot_, key_, session, now))
      clear(slot_);
  }

  void forget() override { clear(slot_); }

  // For a different server or identity than the last: what was kept for the last is not for this one, and is left in
  // the slot only until it is found not to match (load drops it).
  Store *select(const Key &key) override {
    key_ = key;
    return this;
  }

 private:
  Slot &slot_;
  Key key_;
  Now now_;
  int64_t max_age_s_;
};

template <typename Now> SlotStore<Now> make_store(Slot &slot, const Key &key, Now now, int64_t max_age_s = 6 * 3600) {
  return SlotStore<Now>(slot, key, now, max_age_s);
}

}  // namespace eink_session_cache
