#include "check.h"
#include "eink/core/plan.h"

using namespace eink_plan;

static const Screen CLEAN{false, false, false};

TEST(a_changed_image_is_drawn) {
  CHECK(needs_draw(true, false, CLEAN));
  CHECK(needs_draw(true, true, CLEAN));
}

TEST(an_unchanged_image_with_nothing_else_to_change_is_skipped) {
  CHECK(!needs_draw(false, false, CLEAN));
  CHECK(!needs_draw(false, true, CLEAN));
}

TEST(a_notice_over_the_picture_is_cleared_once_the_server_is_fine_again) {
  CHECK(needs_draw(false, false, Screen{true, false, false}));
}

TEST(a_notice_is_not_redrawn_every_wake_while_the_image_is_stale) {
  CHECK(!needs_draw(false, true, Screen{true, false, false}));
}

TEST(the_battery_label_is_drawn_when_it_should_appear_and_when_it_should_go) {
  CHECK(needs_draw(false, false, Screen{false, false, true}));  // low, not yet drawn
  CHECK(needs_draw(false, false, Screen{false, true, false}));  // drawn, no longer low
  CHECK(!needs_draw(false, false, Screen{false, true, true}));  // drawn and still low
}

TEST(only_a_real_offset_is_kept) {
  CHECK(!keeps_offset(false, 3600));  // not a number
  CHECK(keeps_offset(true, 3600));
  CHECK(keeps_offset(true, 0));
  CHECK(keeps_offset(true, -12 * 3600));
  CHECK(keeps_offset(true, 14 * 3600));
  CHECK(!keeps_offset(true, 14 * 3600 + 1));
  CHECK(!keeps_offset(true, -12 * 3600 - 1));
  CHECK(!keeps_offset(true, (int64_t) 1 << 40));  // a value that does not fit an int32_t is no offset
}

TEST(a_pairing_code_the_owner_still_needs_is_left_on_the_panel) {
  CHECK(!needs_draw(false, false, Screen{true, false, false, true}));
  CHECK(!needs_draw(false, false, Screen{true, false, false, true}));
  // Once it is not wanted any more (paired, or the window shut) the notice is cleared like any other.
  CHECK(needs_draw(false, false, Screen{true, false, false, false}));
}

TEST(a_changed_image_is_drawn_whatever_the_prompt) { CHECK(needs_draw(true, false, Screen{true, false, false, true})); }

TEST(the_battery_label_still_wins_over_a_prompt) {
  CHECK(needs_draw(false, false, Screen{true, false, true, true}));  // low, label not drawn yet
}
