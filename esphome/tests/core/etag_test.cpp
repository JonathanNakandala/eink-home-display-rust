#include "check.h"
#include "eink/core/etag.h"
#include "eink/core/plan.h"

using namespace eink_etag;

static const char *SERVER_ETAG = "\"0123456789abcdef0123456789abcdef\"";

static Slot empty_slot() {
  Slot slot;
  clear(slot);
  return slot;
}

TEST(an_etag_that_is_kept_is_given_back) {
  Slot slot = empty_slot();
  keep(slot, SERVER_ETAG);
  CHECK_EQ(kept(slot), SERVER_ETAG);
}

TEST(a_newer_etag_replaces_the_one_before_whole) {
  Slot slot = empty_slot();
  keep(slot, "\"a-much-longer-etag-than-the-one-that-follows\"");
  keep(slot, "\"b\"");
  CHECK_EQ(kept(slot), "\"b\"");
}

TEST(nothing_is_kept_for_a_picture_with_no_etag) {
  Slot slot = empty_slot();
  keep(slot, SERVER_ETAG);
  keep(slot, "");  // the next picture came without one: the old one is no longer the panel's
  CHECK_EQ(kept(slot), "");
}

TEST(what_is_not_an_etag_is_not_kept) {
  Slot slot = empty_slot();
  keep(slot, "has a space");
  CHECK_EQ(kept(slot), "");
  keep(slot, "line\r\nbreak");
  CHECK_EQ(kept(slot), "");
  keep(slot, std::string(CAPACITY + 1, 'x'));
  CHECK_EQ(kept(slot), "");
  keep(slot, std::string(CAPACITY, 'x'));  // as long as it can be
  CHECK_EQ(kept(slot), std::string(CAPACITY, 'x'));
}

TEST(memory_another_firmware_left_is_not_believed) {
  Slot slot = empty_slot();
  keep(slot, SERVER_ETAG);
  slot.magic ^= 1;
  CHECK_EQ(kept(slot), "");

  keep(slot, SERVER_ETAG);
  slot.length = CAPACITY + 1;
  CHECK_EQ(kept(slot), "");

  keep(slot, SERVER_ETAG);
  slot.data[3] = '\n';  // damaged
  CHECK_EQ(kept(slot), "");

  Slot noise;
  for (size_t i = 0; i < sizeof noise; i++)
    reinterpret_cast<unsigned char *>(&noise)[i] = 0xA5;
  CHECK_EQ(kept(noise), "");
}

TEST(the_etag_is_asked_with_only_when_the_panel_shows_just_the_picture) {
  Slot slot = empty_slot();
  keep(slot, SERVER_ETAG);
  CHECK_EQ(condition(slot, true), SERVER_ETAG);
  CHECK_EQ(condition(slot, false), "");
  CHECK_EQ(condition(empty_slot(), true), "");
}

TEST(the_panel_shows_just_the_picture_unless_a_notice_or_a_label_is_on_it) {
  using eink_plan::Screen;
  using eink_plan::shows_only_the_picture;
  CHECK(shows_only_the_picture(Screen{false, false, false}));
  CHECK(shows_only_the_picture(Screen{false, true, true}));    // the label is there and should be
  CHECK(!shows_only_the_picture(Screen{true, false, false}));  // a notice
  CHECK(!shows_only_the_picture(Screen{false, true, false}));  // a label that should go
  CHECK(!shows_only_the_picture(Screen{false, false, true}));  // a label that should appear
}
