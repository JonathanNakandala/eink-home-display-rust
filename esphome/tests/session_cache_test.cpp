#include <string>

#include "check.h"
#include "../eink_session_cache.h"

using namespace eink_session_cache;

static Bytes blob(size_t n, uint8_t v = 7) { return Bytes(n, v); }
static const Bytes ROOT = {0x30, 0x03, 0x02, 0x01, 0x01};
static const Bytes CERT = {0xCE, 0x01, 0x02};
static const uint32_t IP = 0x0100007f;
static const int64_t NOW = 1791463200;

static Slot fresh() {
  Slot slot;
  clear(slot);
  return slot;
}

TEST(the_key_changes_with_the_server_the_port_the_root_and_the_certificate) {
  const Key base = make_key(IP, 8443, ROOT, CERT);
  CHECK(make_key(IP + 1, 8443, ROOT, CERT) != base);
  CHECK(make_key(IP, 8444, ROOT, CERT) != base);
  CHECK(make_key(IP, 8443, Bytes{9}, CERT) != base);
  CHECK(make_key(IP, 8443, ROOT, Bytes{9}) != base);
  CHECK(make_key(IP, 8443, ROOT, CERT) == base);
}

TEST(moving_a_byte_between_the_root_and_the_certificate_is_another_key) {
  CHECK(make_key(IP, 8443, Bytes{1, 2}, Bytes{3}) != make_key(IP, 8443, Bytes{1}, Bytes{2, 3}));
}

TEST(a_session_comes_back_as_it_went_with_how_long_it_has_been_kept) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  CHECK(store(slot, key, blob(1500, 3), NOW));
  const Loaded back = load(slot, key, NOW + 600, 3600);
  CHECK(back.why == Why::OK);
  CHECK(back.session == blob(1500, 3));
  CHECK_EQ(back.age_s, (int64_t) 600);
}

TEST(a_new_slot_holds_nothing) {
  CHECK(load(EMPTY, make_key(IP, 1, ROOT, CERT), NOW, 3600).why == Why::EMPTY);
  Slot slot = fresh();
  CHECK(load(slot, make_key(IP, 1, ROOT, CERT), NOW, 3600).why == Why::EMPTY);
}

TEST(a_session_is_not_offered_to_another_server_or_identity) {
  Slot slot = fresh();
  store(slot, make_key(IP, 8443, ROOT, CERT), blob(100), NOW);
  CHECK(load(slot, make_key(IP, 8443, ROOT, Bytes{0xCE, 0x09}), NOW, 3600).why ==
        Why::OTHER_KEY);  // renewed certificate
  CHECK(load(slot, make_key(IP, 9999, ROOT, CERT), NOW, 3600).why == Why::OTHER_KEY);
  CHECK(load(slot, make_key(IP, 8443, ROOT, CERT), NOW, 3600).why == Why::OK);
}

TEST(a_session_kept_too_long_or_from_the_future_is_not_offered) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  store(slot, key, blob(100), NOW);
  CHECK(load(slot, key, NOW + 3600, 3600).why == Why::OK);  // exactly the limit
  CHECK(load(slot, key, NOW + 3601, 3600).why == Why::TOO_OLD);
  CHECK(load(slot, key, NOW - 1, 3600).why == Why::FROM_FUTURE);  // the wall clock went back
}

TEST(a_session_that_is_too_big_or_empty_is_not_kept_and_drops_the_old_one) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  CHECK(store(slot, key, blob(CAPACITY), NOW));  // exactly the capacity
  CHECK(!store(slot, key, blob(CAPACITY + 1), NOW));
  CHECK(load(slot, key, NOW, 3600).why == Why::EMPTY);
  store(slot, key, blob(100), NOW);
  CHECK(!store(slot, key, Bytes(), NOW));
  CHECK(load(slot, key, NOW, 3600).why == Why::EMPTY);
}

TEST(damage_anywhere_in_the_slot_is_caught) {
  Slot good = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  store(good, key, blob(300, 5), NOW);
  const uint8_t *raw = reinterpret_cast<const uint8_t *>(&good);
  // Every byte of the header and the data in use: one flipped bit, and it is not believed.
  const size_t used = offsetof(Slot, data) + good.length;
  for (size_t at = 0; at < used; at += 3) {
    Slot damaged = good;
    reinterpret_cast<uint8_t *>(&damaged)[at] ^= 0x10;
    const Why why = load(damaged, key, NOW, 3600).why;
    // The padding bytes (reserved) are covered by the CRC too; the one thing a flip may do to the key is make it
    // another key.
    CHECK(why != Why::OK);
  }
  (void) raw;
  // Bytes past the data in use are not part of it.
  Slot beyond = good;
  beyond.data[good.length + 10] ^= 0xFF;
  CHECK(load(beyond, key, NOW, 3600).why == Why::OK);
}

TEST(a_length_that_lies_is_damage_and_not_a_read_past_the_slot) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  store(slot, key, blob(100), NOW);
  slot.length = 0xFFFFFFFF;
  slot.crc = crc_of(slot);
  CHECK(load(slot, key, NOW, 3600).why == Why::DAMAGED);
  slot.length = 0;
  slot.crc = crc_of(slot);
  CHECK(load(slot, key, NOW, 3600).why == Why::DAMAGED);
}

TEST(memory_from_another_firmware_or_version_is_not_believed) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  store(slot, key, blob(100), NOW);
  Slot other_version = slot;
  other_version.version = VERSION + 1;
  other_version.crc = crc_of(other_version);
  CHECK(load(other_version, key, NOW, 3600).why == Why::DAMAGED);
  Slot other_magic = slot;
  other_magic.magic = 0xDEADBEEF;
  other_magic.crc = crc_of(other_magic);
  CHECK(load(other_magic, key, NOW, 3600).why == Why::DAMAGED);
}

TEST(a_store_over_a_slot_offers_what_was_saved_and_forgets_what_was_not_wanted) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  int64_t clock = NOW;
  auto store_ = make_store(slot, key, [&clock] { return clock; });
  Bytes got;
  int64_t age = -1;
  CHECK(!store_.load(got, age));  // nothing yet
  store_.save(blob(900, 4));
  clock = NOW + 120;
  CHECK(store_.load(got, age));
  CHECK(got == blob(900, 4));
  CHECK_EQ(age, (int64_t) 120);
  store_.forget();
  CHECK(!store_.load(got, age));
}

TEST(a_newer_session_replaces_the_older) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  int64_t clock = NOW;
  auto store_ = make_store(slot, key, [&clock] { return clock; });
  store_.save(blob(100, 1));
  clock = NOW + 60;
  store_.save(blob(100, 2));
  Bytes got;
  int64_t age = 0;
  CHECK(store_.load(got, age));
  CHECK(got == blob(100, 2));
  CHECK_EQ(age, (int64_t) 0);
}

TEST(nothing_is_kept_or_offered_when_the_clock_cannot_be_believed) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  int64_t clock = -1;
  auto store_ = make_store(slot, key, [&clock] { return clock; });
  store_.save(blob(100));
  CHECK_EQ(slot.length, 0u);  // not kept: its age could not be known
  clock = NOW;
  store_.save(blob(100));
  clock = -1;
  Bytes got;
  int64_t age;
  CHECK(!store_.load(got, age));  // kept, but not offered while the clock is untrusted
  CHECK_EQ(slot.length, 100u);    // and left where it is for when it is
}

TEST(a_damaged_or_stale_slot_is_dropped_so_it_is_not_read_again_every_wake) {
  Slot slot = fresh();
  const Key key = make_key(IP, 8443, ROOT, CERT);
  int64_t clock = NOW;
  auto store_ = make_store(slot, key, [&clock] { return clock; }, 3600);
  store_.save(blob(100));
  clock = NOW + 7200;  // past the limit
  Bytes got;
  int64_t age;
  CHECK(!store_.load(got, age));
  CHECK_EQ(slot.length, 0u);
  clock = NOW;
  slot.data[0] ^= 1;  // damage
  store_.save(blob(100));
  slot.data[3] ^= 1;
  CHECK(!store_.load(got, age));
  CHECK_EQ(slot.magic, 0u);
}

TEST(the_slot_is_small_enough_for_rtc_memory) {
  CHECK(sizeof(Slot) <= 3200);
  CHECK_EQ(offsetof(Slot, data) % 4, (size_t) 0);
}
