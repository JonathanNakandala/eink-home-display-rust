#include <string>

#include "check.h"
#include "fake_store.h"
#include "eink/core/credentials.h"

using namespace eink_credentials;
using fakes::MemoryStore;
using State = Credentials::State;

static Bytes filled(size_t n, uint8_t v) { return Bytes(n, v); }

static Record sample() {
  Record r;
  r.key = filled(121, 0x11);
  r.root = filled(417, 0x22);
  r.certificate = filled(523, 0x33);
  r.not_before = 1791459600;
  r.not_after = 1799235600;
  r.sequence = 7;
  return r;
}

TEST(the_checksum_is_the_one_zip_and_ethernet_use) {
  const std::string check = "123456789";
  CHECK_EQ(crc32(reinterpret_cast<const uint8_t *>(check.data()), check.size()), 0xCBF43926u);
  CHECK_EQ(crc32(nullptr, 0), 0u);
}

TEST(a_record_comes_back_as_it_went) {
  const Record in = sample();
  Record out;
  CHECK(decode(encode(in), out) == Decode::OK);
  CHECK(out.key == in.key && out.root == in.root && out.certificate == in.certificate);
  CHECK_EQ(out.not_before, in.not_before);
  CHECK_EQ(out.not_after, in.not_after);
  CHECK_EQ(out.sequence, 7u);
}

TEST(a_record_with_nothing_in_the_fields_comes_back_too) {
  Record in;
  in.sequence = 1;
  Record out;
  CHECK(decode(encode(in), out) == Decode::OK);
  CHECK(out.key.empty() && out.root.empty() && out.certificate.empty());
}

TEST(dates_before_1970_and_the_largest_fields_survive) {
  Record in;
  in.sequence = 0xFFFFFFFFu;
  in.not_before = -1;
  in.not_after = INT64_MAX;
  in.certificate = filled(MAX_FIELD, 0xAB);
  Record out;
  CHECK(decode(encode(in), out) == Decode::OK);
  CHECK_EQ(out.not_before, (int64_t) -1);
  CHECK_EQ(out.not_after, INT64_MAX);
  CHECK_EQ(out.certificate.size(), MAX_FIELD);
  in.certificate = filled(MAX_FIELD + 1, 0);
  CHECK(encode(in).empty());  // too long to describe: not written
}

TEST(any_cut_of_a_record_is_not_a_record) {
  const Bytes whole = encode(sample());
  for (size_t cut = 0; cut < whole.size(); cut++) {
    Record out;
    CHECK(decode(Bytes(whole.begin(), whole.begin() + static_cast<long>(cut)), out) == Decode::BAD);
  }
}

TEST(any_single_flipped_bit_is_caught) {
  const Bytes whole = encode(sample());
  for (size_t byte = 0; byte < whole.size(); byte++) {
    for (int bit = 0; bit < 8; bit += 3) {
      Bytes damaged = whole;
      damaged[byte] ^= static_cast<uint8_t>(1u << bit);
      Record out;
      CHECK(decode(damaged, out) != Decode::OK);
    }
  }
}

TEST(a_record_of_another_version_is_recognised_and_not_taken_for_damage) {
  Bytes other = encode(sample());
  other[4] = VERSION + 1;
  const uint32_t crc = crc32(other.data(), other.size() - 4);  // a firmware that wrote it would have made this
  for (int i = 0; i < 4; i++)
    other[other.size() - 4 + static_cast<size_t>(i)] = static_cast<uint8_t>(crc >> (8 * i));
  Record out;
  CHECK(decode(other, out) == Decode::UNSUPPORTED);
}

TEST(a_record_whose_lengths_lie_is_not_a_record) {
  Bytes lie = encode(sample());
  lie[10] = 0xFF;  // the key's length, with the checksum put right
  lie[11] = 0xFF;
  const uint32_t crc = crc32(lie.data(), lie.size() - 4);
  for (int i = 0; i < 4; i++)
    lie[lie.size() - 4 + static_cast<size_t>(i)] = static_cast<uint8_t>(crc >> (8 * i));
  Record out;
  CHECK(decode(lie, out) == Decode::BAD);
}

TEST(a_new_display_has_nothing) {
  MemoryStore store;
  Credentials c(store);
  CHECK(c.state() == State::EMPTY);
  CHECK(c.record().empty());
}

TEST(the_first_record_goes_to_one_slot_and_the_next_to_the_other) {
  MemoryStore store;
  Credentials c(store);
  Record r;
  r.key = filled(10, 1);
  CHECK(c.update(r));
  CHECK_EQ(store.blobs.count("cred_a"), (size_t) 1);
  CHECK_EQ(store.blobs.count("cred_b"), (size_t) 0);
  r.root = filled(10, 2);
  CHECK(c.update(r));
  CHECK_EQ(store.blobs.count("cred_b"), (size_t) 1);
  r.certificate = filled(10, 3);
  CHECK(c.update(r));
  CHECK_EQ(c.record().sequence, 3u);
  // Three writes: a, b, a.
  Record in_a, in_b;
  CHECK(decode(store.blobs["cred_a"], in_a) == Decode::OK);
  CHECK(decode(store.blobs["cred_b"], in_b) == Decode::OK);
  CHECK_EQ(in_a.sequence, 3u);
  CHECK_EQ(in_b.sequence, 2u);
}

TEST(what_was_kept_is_there_after_a_restart) {
  MemoryStore store;
  {
    Credentials c(store);
    Record r;
    r.key = filled(121, 5);
    c.update(r);
    r.root = filled(417, 6);
    c.update(r);
  }
  Credentials again(store);
  CHECK(again.state() == State::READY);
  CHECK(again.record().key == filled(121, 5));
  CHECK(again.record().root == filled(417, 6));
  CHECK_EQ(again.record().sequence, 2u);
}

TEST(after_a_restart_the_next_write_goes_to_the_other_slot_than_the_latest) {
  MemoryStore store;
  {
    Credentials c(store);
    Record r;
    r.key = filled(8, 1);
    c.update(r);  // a
    c.update(r);  // b
  }
  Credentials again(store);  // the latest is b
  Record r = again.record();
  r.root = filled(8, 9);
  CHECK(again.update(r));  // so a
  Record in_a;
  CHECK(decode(store.blobs["cred_a"], in_a) == Decode::OK);
  CHECK_EQ(in_a.sequence, 3u);
}

TEST(a_write_that_fails_changes_nothing) {
  MemoryStore store;
  Credentials c(store);
  Record r;
  r.key = filled(8, 1);
  CHECK(c.update(r));
  r.root = filled(8, 2);
  store.fail_write = 1;
  CHECK(!c.update(r));
  CHECK(c.record().root.empty());  // not what was asked, still what was there
  CHECK_EQ(c.record().sequence, 1u);
  Credentials again(store);
  CHECK(again.record().root.empty());
  CHECK(c.update(r));  // and it can be tried again
  CHECK_EQ(c.record().sequence, 2u);
}

// The heart of it: a power cut at any write of a long series leaves a record that was once whole, never a mixture, and
// the next write after the power is back works.
TEST(a_power_cut_in_any_write_leaves_the_record_before_it_or_the_one_after) {
  for (int cut = 1; cut <= 6; cut++) {
    MemoryStore store;
    std::vector<Record> history;
    {
      Credentials c(store);
      Record r;
      for (int step = 1; step <= 6; step++) {
        r.key = filled(121, 1);
        if (step >= 2)
          r.root = filled(417, 2);
        if (step >= 3) {
          r.certificate = filled(523, static_cast<uint8_t>(step));
          r.not_after = step * 1000;
        }
        if (step == cut)
          store.cut_write = 1;
        const bool done = c.update(r);
        if (done)
          history.push_back(c.record());
        else
          break;  // the power went
      }
    }
    store.restore_power();
    Credentials after(store);
    if (history.empty()) {
      // The very first write was cut: nothing whole was ever kept, and what is there is not a record.
      CHECK(after.state() == State::UNREADABLE || after.state() == State::EMPTY);
      continue;
    }
    CHECK(after.state() == State::READY);
    CHECK_EQ(after.record().sequence, history.back().sequence);
    CHECK(after.record().key == history.back().key);
    CHECK(after.record().root == history.back().root);
    CHECK(after.record().certificate == history.back().certificate);
    // And it goes on from there, writing over the damaged slot.
    Record next = after.record();
    next.not_after = 424242;
    CHECK(after.update(next));
    Credentials last(store);
    CHECK_EQ(last.record().not_after, (int64_t) 424242);
    CHECK_EQ(last.record().sequence, history.back().sequence + 1);
  }
}

TEST(a_display_whose_first_write_was_cut_short_is_not_taken_for_a_new_one) {
  MemoryStore store;
  {
    Credentials c(store);
    Record r;
    r.key = filled(121, 1);
    store.cut_write = 1;
    CHECK(!c.update(r));
  }
  store.restore_power();
  Credentials after(store);
  // Something is there and it is not a record: it is left alone, and nothing is written over it.
  CHECK(after.state() == State::UNREADABLE);
  Record r;
  r.key = filled(5, 5);
  CHECK(!after.update(r));
}

TEST(a_flash_that_cannot_be_read_is_never_taken_for_an_empty_one) {
  MemoryStore store;
  {
    Credentials c(store);
    Record r;
    r.key = filled(121, 1);
    c.update(r);
  }
  for (const char *broken : {"cred_a", "cred_b"}) {
    store.broken_reads.clear();
    store.broken_reads[broken] = true;
    Credentials c(store);
    CHECK(c.state() == State::UNREADABLE);
    const int writes = store.writes;
    Record r;
    r.key = filled(8, 8);
    CHECK(!c.update(r));
    CHECK_EQ(store.writes, writes);  // nothing was written over what could not be read
  }
}

TEST(a_record_another_firmware_wrote_is_left_alone) {
  MemoryStore store;
  Bytes other = encode(sample());
  other[4] = VERSION + 1;
  const uint32_t crc = crc32(other.data(), other.size() - 4);
  for (int i = 0; i < 4; i++)
    other[other.size() - 4 + static_cast<size_t>(i)] = static_cast<uint8_t>(crc >> (8 * i));
  store.blobs["cred_a"] = other;
  Credentials c(store);
  CHECK(c.state() == State::UNREADABLE);
  Record r;
  r.key = filled(8, 8);
  CHECK(!c.update(r));
  CHECK(store.blobs["cred_a"] == other);
}

TEST(damage_to_every_slot_is_not_an_empty_display_either) {
  MemoryStore store;
  store.blobs["cred_a"] = Bytes{1, 2, 3};
  store.blobs["cred_b"] = Bytes{4, 5, 6};
  Credentials c(store);
  CHECK(c.state() == State::UNREADABLE);
}

TEST(a_damaged_slot_beside_a_good_one_is_the_one_written_over) {
  MemoryStore store;
  {
    Credentials c(store);
    Record r;
    r.key = filled(8, 1);
    c.update(r);  // a, sequence 1
    r.root = filled(8, 2);
    c.update(r);  // b, sequence 2
  }
  store.blobs["cred_a"][3] ^= 0x40;  // bit rot in the older one
  Credentials c(store);
  CHECK(c.state() == State::READY);
  CHECK_EQ(c.record().sequence, 2u);
  Record r = c.record();
  r.certificate = filled(8, 3);
  CHECK(c.update(r));  // over the damaged one
  Record in_a;
  CHECK(decode(store.blobs["cred_a"], in_a) == Decode::OK);
  CHECK_EQ(in_a.sequence, 3u);
}

TEST(the_later_of_two_good_records_wins_even_across_the_sequence_wrapping) {
  MemoryStore store;
  Record a = sample(), b = sample();
  a.sequence = 0xFFFFFFFFu;
  b.sequence = 1;  // four billion writes on, wrapped
  b.not_after = 99;
  store.blobs["cred_a"] = encode(a);
  store.blobs["cred_b"] = encode(b);
  Credentials c(store);
  CHECK_EQ(c.record().not_after, (int64_t) 99);
}

TEST(erasing_removes_the_record_and_leaves_a_display_that_is_new) {
  MemoryStore store;
  Credentials credentials(store);
  Record record;
  record.key = filled(10, 1);
  record.certificate = filled(20, 2);
  CHECK(credentials.update(record));
  CHECK(credentials.update(record));  // both slots hold one
  CHECK_EQ(store.blobs.size(), 2u);

  CHECK(credentials.erase());
  CHECK(credentials.state() == State::EMPTY);
  CHECK(credentials.record().empty());
  CHECK(store.blobs.empty());

  // And a restart agrees, and the display can be paired afresh.
  Credentials after(store);
  CHECK(after.state() == State::EMPTY);
  CHECK(after.update(record));
  CHECK(after.state() == State::READY);
}

TEST(the_slot_that_is_not_current_is_erased_first_so_a_cut_leaves_the_display_as_it_was) {
  MemoryStore store;
  Credentials credentials(store);
  Record record;
  record.key = filled(10, 1);
  CHECK(credentials.update(record));  // slot A
  record.certificate = filled(20, 3);
  CHECK(credentials.update(record));  // slot B, the current one

  store.cut_erase = 2;  // the power goes just before the second erase
  CHECK(!credentials.erase());
  CHECK_EQ(store.erased.size(), 1u);
  CHECK_EQ(store.erased[0], std::string("cred_a"));
  // Still paired with the record it had, in this run and after a restart; the owner can hold the button again.
  CHECK(credentials.state() == State::READY);
  store.restore_power();
  Credentials after(store);
  CHECK(after.state() == State::READY);
  CHECK(after.record().certificate == filled(20, 3));
}

TEST(an_erase_that_fails_changes_nothing_in_what_the_display_holds) {
  MemoryStore store;
  Credentials credentials(store);
  Record record;
  record.key = filled(10, 1);
  CHECK(credentials.update(record));
  store.fail_erase = 1;
  CHECK(!credentials.erase());
  CHECK(credentials.state() == State::READY);
  CHECK(!credentials.record().key.empty());
}

TEST(a_record_that_cannot_be_read_can_still_be_erased_by_the_owner) {
  // The way out of a flash that holds something unreadable: the one thing that removes it is the owner asking.
  MemoryStore store;
  store.blobs["cred_a"] = filled(30, 9);  // not a record
  Credentials credentials(store);
  CHECK(credentials.state() == State::UNREADABLE);
  CHECK(!credentials.update(Record()));  // an error never writes over it
  CHECK(credentials.erase());
  CHECK(credentials.state() == State::EMPTY);
  CHECK(store.blobs.empty());
  Record record;
  record.key = filled(10, 1);
  CHECK(credentials.update(record));
}

TEST(erasing_a_display_that_has_nothing_is_fine) {
  MemoryStore store;
  Credentials credentials(store);
  CHECK(credentials.erase());
  CHECK(credentials.state() == State::EMPTY);
}
