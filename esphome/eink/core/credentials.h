// What the display keeps in flash to stay paired: its key, the root it trusts, its certificate and the dates of it.
// Kept as one record, written whole to one of two slots and read back with a version, a sequence number and a checksum,
// so that a write cut short by a power loss leaves the record before it, and nothing is ever half one thing and half
// the other.
//
// Works over a `BlobStore` (what flash does: read and write a named blob), and is tested with every way a write can be
// cut short.
//
// Pairing is removed only by the owner (holding the button, or erasing the flash), never by an error. So a record that
// cannot be read for any reason but there being none (a failing flash, a bad checksum on every slot, a version written
// by another firmware) is not "empty": it stays untouched, and nothing is written over it.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace eink_credentials {

using Bytes = std::vector<uint8_t>;

// ---- what flash does ------------------------------------------------------------------------------------------------

enum class Read : uint8_t {
  OK,
  ABSENT,  // there is no such blob
  ERROR,   // the flash could not be read: what is there is unknown
};

class BlobStore {
 public:
  virtual ~BlobStore() = default;
  virtual Read read(const char *name, Bytes &out) = 0;
  // Removes the blob. True if it is gone, including when there never was one; false if it may still be there.
  virtual bool erase(const char *name) = 0;
  // All or nothing as far as the caller can tell. A write that returns false may have changed nothing, or may have left
  // the blob cut short (a power cut): the record format is what makes that safe.
  virtual bool write(const char *name, const Bytes &data) = 0;
};

// ---- the record -----------------------------------------------------------------------------------------------------

struct Record {
  Bytes key;          // the private key, DER
  Bytes root;         // the root a certificate was issued under, DER; empty until one was
  Bytes certificate;  // DER; empty until one was issued
  int64_t not_before = 0;
  int64_t not_after = 0;
  uint32_t sequence = 0;  // 0: there is no record yet

  bool empty() const { return sequence == 0; }
};

// CRC-32 as zip and ethernet use it (reflected, polynomial 0xEDB88320), bit by bit: small, and fast enough for a few
// KB.
inline uint32_t crc32(const uint8_t *data, size_t length) {
  uint32_t crc = 0xFFFFFFFFu;
  for (size_t i = 0; i < length; i++) {
    crc ^= data[i];
    for (int bit = 0; bit < 8; bit++)
      crc = (crc >> 1) ^ (0xEDB88320u & (0u - (crc & 1u)));
  }
  return ~crc;
}

constexpr uint8_t VERSION = 1;
constexpr size_t MAX_FIELD = 0xFFFF;

inline void put16(Bytes &out, uint16_t v) {
  out.push_back(static_cast<uint8_t>(v));
  out.push_back(static_cast<uint8_t>(v >> 8));
}
inline void put32(Bytes &out, uint32_t v) {
  for (int i = 0; i < 4; i++)
    out.push_back(static_cast<uint8_t>(v >> (8 * i)));
}
inline void put64(Bytes &out, uint64_t v) {
  for (int i = 0; i < 8; i++)
    out.push_back(static_cast<uint8_t>(v >> (8 * i)));
}

// "EINK", the version, a reserved byte, the sequence, then each of key, root and certificate with its length, the two
// dates, and the CRC-32 of all that comes before it. Little-endian throughout. Empty if a field is too long to
// describe.
inline Bytes encode(const Record &record) {
  if (record.key.size() > MAX_FIELD || record.root.size() > MAX_FIELD || record.certificate.size() > MAX_FIELD)
    return {};
  Bytes out = {'E', 'I', 'N', 'K', VERSION, 0};
  put32(out, record.sequence);
  for (const Bytes *field : {&record.key, &record.root, &record.certificate}) {
    put16(out, static_cast<uint16_t>(field->size()));
    out.insert(out.end(), field->begin(), field->end());
  }
  put64(out, static_cast<uint64_t>(record.not_before));
  put64(out, static_cast<uint64_t>(record.not_after));
  put32(out, crc32(out.data(), out.size()));
  return out;
}

enum class Decode : uint8_t {
  OK,
  BAD,          // not a record: cut short, the wrong checksum, or the wrong shape
  UNSUPPORTED,  // a record, of a version this firmware does not know
};

inline Decode decode(const Bytes &data, Record &record) {
  constexpr size_t FIXED = 4 + 1 + 1 + 4 + 3 * 2 + 8 + 8 + 4;  // the record with every field empty
  if (data.size() < FIXED || data[0] != 'E' || data[1] != 'I' || data[2] != 'N' || data[3] != 'K')
    return Decode::BAD;
  // The checksum first: nothing in a record that fails it is believed, not even its version.
  const size_t body = data.size() - 4;
  uint32_t stored = 0;
  for (int i = 0; i < 4; i++)
    stored |= static_cast<uint32_t>(data[body + static_cast<size_t>(i)]) << (8 * i);
  if (stored != crc32(data.data(), body))
    return Decode::BAD;
  if (data[4] != VERSION)
    return Decode::UNSUPPORTED;

  Record out;
  size_t at = 6;
  const auto u32 = [&](uint32_t &v) {
    if (at + 4 > body)
      return false;
    v = 0;
    for (int i = 0; i < 4; i++)
      v |= static_cast<uint32_t>(data[at + static_cast<size_t>(i)]) << (8 * i);
    at += 4;
    return true;
  };
  const auto u64 = [&](int64_t &v) {
    if (at + 8 > body)
      return false;
    uint64_t x = 0;
    for (int i = 0; i < 8; i++)
      x |= static_cast<uint64_t>(data[at + static_cast<size_t>(i)]) << (8 * i);
    v = static_cast<int64_t>(x);
    at += 8;
    return true;
  };
  const auto field = [&](Bytes &v) {
    if (at + 2 > body)
      return false;
    const size_t n = data[at] | (static_cast<size_t>(data[at + 1]) << 8);
    at += 2;
    if (at + n > body)
      return false;
    v.assign(data.begin() + static_cast<long>(at), data.begin() + static_cast<long>(at + n));
    at += n;
    return true;
  };
  if (!u32(out.sequence) || !field(out.key) || !field(out.root) || !field(out.certificate) || !u64(out.not_before) ||
      !u64(out.not_after) || at != body || out.sequence == 0)
    return Decode::BAD;
  record = out;
  return Decode::OK;
}

// ---- two slots ------------------------------------------------------------------------------------------------------

class Credentials {
 public:
  enum class State : uint8_t {
    EMPTY,       // nothing has ever been kept: a new display
    READY,       // a record, in `record()`
    UNREADABLE,  // something is there that cannot be read: left alone, and nothing is written
  };

  explicit Credentials(BlobStore &store) : store_(store) { load(); }

  State state() const { return state_; }
  const Record &record() const { return record_; }

  // Removes the record, whatever state it is in (a record that cannot be read is no reason to keep it), because the
  // owner asked: this is the one thing that removes pairing. The slot that is not current goes first, so that if the
  // power goes in between, the display is still paired with what it had and the owner can ask again; going the other
  // way the older record could come back. True if both are gone; then the state is EMPTY, as for a new display. False
  // if one would not go, and the state is as it was.
  bool erase() {
    const int first = active_ == 0 ? 1 : 0;
    if (!store_.erase(name(first)) || !store_.erase(name(1 - first)))
      return false;
    record_ = Record();
    active_ = -1;
    state_ = State::EMPTY;
    return true;
  }

  // Writes `next` as the new record, to the slot that is not the current one, and makes it current only if the write
  // was made. The sequence is set here. False if the flash would not take it, or if the state is UNREADABLE.
  bool update(Record next) {
    if (state_ == State::UNREADABLE)
      return false;
    next.sequence = record_.sequence + 1;
    if (next.sequence == 0)
      next.sequence = 1;  // wrapped after four billion writes
    const Bytes bytes = encode(next);
    if (bytes.empty())
      return false;
    const int slot = active_ == 0 ? 1 : 0;  // the first record goes to slot 0; after that they take turns
    if (!store_.write(name(slot), bytes))
      return false;
    record_ = next;
    active_ = slot;
    state_ = State::READY;
    return true;
  }

 private:
  BlobStore &store_;
  State state_ = State::EMPTY;
  Record record_;
  int active_ = -1;  // the slot holding record_; none yet

  static const char *name(int slot) { return slot == 0 ? "cred_a" : "cred_b"; }

  void load() {
    Record found[2];
    Decode decoded[2] = {Decode::BAD, Decode::BAD};
    bool present[2] = {false, false};
    for (int slot = 0; slot < 2; slot++) {
      Bytes bytes;
      const Read r = store_.read(name(slot), bytes);
      if (r == Read::ERROR) {
        state_ = State::UNREADABLE;  // unknown: not guessed at, and so not written over
        return;
      }
      if (r == Read::OK) {
        present[slot] = true;
        decoded[slot] = decode(bytes, found[slot]);
        if (decoded[slot] == Decode::UNSUPPORTED) {
          state_ = State::UNREADABLE;  // another firmware's: never overwritten by this one
          return;
        }
      }
    }
    const bool good0 = decoded[0] == Decode::OK && present[0], good1 = decoded[1] == Decode::OK && present[1];
    if (good0 || good1) {
      // The later of the two. A slot that fails its checksum next to a good one is a write that was cut short; it is
      // the one the next update overwrites.
      active_ = !good1 ? 0 : !good0 ? 1 : (static_cast<int32_t>(found[1].sequence - found[0].sequence) > 0 ? 1 : 0);
      record_ = found[active_];
      state_ = State::READY;
    } else if (!present[0] && !present[1]) {
      state_ = State::EMPTY;
    } else {
      state_ = State::UNREADABLE;  // something is there, and none of it is a record
    }
  }
};

}  // namespace eink_credentials
