#include "check.h"
#include "home_display/core/trust.h"

using namespace home_display_trust;

TEST(a_compiled_in_root_wins_and_is_never_replaced) {
  CHECK(root_source(true, false) == RootSource::COMPILED);
  CHECK(root_source(true, true) == RootSource::COMPILED);
  CHECK(root_source(false, true) == RootSource::STORED);
  CHECK(root_source(false, false) == RootSource::NONE);
}

TEST(only_a_display_with_no_root_fetches_one) {
  CHECK(needs_root_fetched(RootSource::NONE));
  CHECK(!needs_root_fetched(RootSource::STORED));
  CHECK(!needs_root_fetched(RootSource::COMPILED));
}

// 90 days, as the server issues.
static const int64_t START = 1791463200;
static const int64_t DAY = 86400;
static const int64_t END = START + 90 * DAY;

TEST(a_fresh_certificate_is_valid_until_a_third_of_its_life_is_left) {
  CHECK(standing(START, START, END) == Standing::VALID);
  CHECK(standing(START + 59 * DAY, START, END) == Standing::VALID);
  CHECK(standing(START + 60 * DAY - 1, START, END) == Standing::VALID);
  CHECK(standing(START + 60 * DAY, START, END) == Standing::DUE);  // 30 days left of 90
  CHECK(standing(START + 89 * DAY, START, END) == Standing::DUE);
}

TEST(the_end_is_the_end) {
  CHECK(standing(END - 1, START, END) == Standing::DUE);
  CHECK(standing(END, START, END) == Standing::EXPIRED);
  CHECK(standing(END + 365 * DAY, START, END) == Standing::EXPIRED);
}

TEST(a_clock_before_the_certificate_starts_is_the_clock_s_fault) {
  CHECK(standing(START - 1, START, END) == Standing::NOT_YET);
  CHECK(standing(0, START, END) == Standing::NOT_YET);
}

TEST(a_certificate_with_no_life_is_over_at_once) { CHECK(standing(START, START, START) == Standing::EXPIRED); }
