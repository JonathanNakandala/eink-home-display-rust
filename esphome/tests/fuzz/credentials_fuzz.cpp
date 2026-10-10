// The record in flash (core/credentials.h) as whatever the flash holds: damaged, from another firmware, or noise. The
// first byte says where to cut the rest into the two slots.
#include <algorithm>
#include <string>

#include "fuzz.h"

#include "home_display/core/credentials.h"
#include "fake_store.h"

using namespace home_display_credentials;

namespace {

bool same(const Record &a, const Record &b) {
  return a.key == b.key && a.root == b.root && a.certificate == b.certificate && a.not_before == b.not_before &&
         a.not_after == b.not_after && a.sequence == b.sequence;
}

}  // namespace

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  if (size == 0)
    return 0;
  const Bytes all(data + 1, data + size);

  // A record that decodes is a record: it is written out and read back as it is.
  Record record;
  if (decode(all, record) == Decode::OK) {
    FUZZ_REQUIRE(record.sequence != 0);
    Record again;
    FUZZ_REQUIRE(decode(encode(record), again) == Decode::OK);
    FUZZ_REQUIRE(same(record, again));
  }

  // Two slots of anything: the state is one of three, a record that is given is a good one, and the owner can always
  // erase it and start again.
  const size_t cut = std::min<size_t>(all.size(), data[0] * all.size() / 255);
  fakes::MemoryStore store;
  if (data[0] & 1)
    store.blobs["cred_a"] = Bytes(all.begin(), all.begin() + static_cast<long>(cut));
  if (data[0] & 2)
    store.blobs["cred_b"] = Bytes(all.begin() + static_cast<long>(cut), all.end());
  Credentials credentials(store);
  switch (credentials.state()) {
    case Credentials::State::READY: FUZZ_REQUIRE(!credentials.record().empty()); break;
    case Credentials::State::EMPTY:
      FUZZ_REQUIRE(credentials.record().empty());
      FUZZ_REQUIRE(store.blobs.empty());  // nothing is empty that holds something
      break;
    case Credentials::State::UNREADABLE:
      FUZZ_REQUIRE(!credentials.update(Record()));  // left alone, never written over
      break;
  }
  FUZZ_REQUIRE(credentials.erase());
  FUZZ_REQUIRE(credentials.state() == Credentials::State::EMPTY);
  Record fresh;
  fresh.key = Bytes(8, 1);
  FUZZ_REQUIRE(credentials.update(fresh));
  Credentials after(store);
  FUZZ_REQUIRE(after.state() == Credentials::State::READY);
  FUZZ_REQUIRE(same(after.record(), credentials.record()));
  return 0;
}
