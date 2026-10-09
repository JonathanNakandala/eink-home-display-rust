// What RTC memory may hold after an update over the air or a brown-out: a TLS session slot (core/session_cache.h) and
// an ETag slot (core/etag.h) made of anything at all. Neither is believed until it is checked, and neither may read
// outside itself while it is.
#include <cstring>
#include <string>

#include "fuzz.h"

#include "eink/core/etag.h"
#include "eink/core/session_cache.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  // The session slot, from the first bytes.
  eink_session_cache::Slot slot;
  std::memset(&slot, 0, sizeof slot);
  std::memcpy(&slot, data, std::min(size, sizeof slot));
  eink_session_cache::Key other{};
  for (const eink_session_cache::Key &key : {slot.key, other}) {
    for (const int64_t now : {int64_t{0}, slot.saved_at, slot.saved_at + 1, int64_t{1791463200}}) {
      const eink_session_cache::Loaded loaded = eink_session_cache::load(slot, key, now, 6 * 3600);
      if (loaded.why == eink_session_cache::Why::OK) {
        FUZZ_REQUIRE(loaded.session.size() == slot.length);
        FUZZ_REQUIRE(slot.length > 0 && slot.length <= eink_session_cache::CAPACITY);
        FUZZ_REQUIRE(loaded.age_s >= 0 && loaded.age_s <= 6 * 3600);
        FUZZ_REQUIRE(slot.key == key);
      } else {
        FUZZ_REQUIRE(loaded.session.empty());
      }
    }
  }
  // A store over it drops what is damaged and does not keep reading it.
  eink_session_cache::Slot held = slot;
  auto store = eink_session_cache::make_store(held, slot.key, [] { return int64_t{1791463200}; });
  eink_session_cache::Bytes session;
  int64_t age = 0;
  if (!store.load(session, age))
    FUZZ_REQUIRE(session.empty());

  // The ETag slot, from the last bytes.
  eink_etag::Slot etag;
  std::memset(&etag, 0, sizeof etag);
  const size_t tail = size > sizeof etag ? size - sizeof etag : 0;
  std::memcpy(&etag, data + tail, std::min(size - tail, sizeof etag));
  const std::string kept = eink_etag::kept(etag);
  FUZZ_REQUIRE(kept.size() <= eink_etag::CAPACITY);
  FUZZ_REQUIRE(kept.empty() || eink_etag::valid(kept));
  FUZZ_REQUIRE(eink_etag::condition(etag, false).empty());
  return 0;
}
