#include <cstdint>

#include "check.h"
#include "../eink_clock.h"

using namespace eink_clock;

TEST(a_clock_that_never_was_set_is_not_plausible) {
  CHECK(!plausible(0));
  CHECK(!plausible(1700000000));  // November 2023
  CHECK(!plausible(-1));
}

TEST(the_earliest_plausible_time_is_the_start_of_2026) {
  CHECK_EQ(EARLIEST_PLAUSIBLE, (int64_t) 1767225600);
  CHECK(!plausible(EARLIEST_PLAUSIBLE - 1));
  CHECK(plausible(EARLIEST_PLAUSIBLE));
  CHECK(plausible(1791463200));  // the server's example render time
}

TEST(a_clock_behind_a_render_that_already_happened_is_not_usable) {
  const int64_t now = 1791463200;
  CHECK(usable(now, 0));  // no render known yet
  CHECK(usable(now, 1791463200));
  CHECK(usable(now, 1791463199));
  CHECK(!usable(now, 1791463201));  // the render is a second in the future
  CHECK(!usable(now, 1900000000));
}

TEST(an_implausible_clock_is_not_usable_whatever_was_rendered) {
  CHECK(!usable(0, 0));
  CHECK(!usable(1000, 0));
  CHECK(!usable(EARLIEST_PLAUSIBLE - 1, 0));
}

TEST(the_largest_render_version_does_not_wrap) {
  CHECK(!usable(1791463200, UINT32_MAX));  // 2106: far ahead of now
  CHECK(usable((int64_t) UINT32_MAX + 1, UINT32_MAX));
}
