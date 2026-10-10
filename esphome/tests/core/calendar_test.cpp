#include "check.h"
#include "home_display/core/calendar.h"

using namespace home_display_calendar;

TEST(the_epoch_is_zero) { CHECK_EQ(epoch_seconds(1970, 1, 1, 0, 0, 0), (int64_t) 0); }

TEST(known_moments) {
  // The server's example render time, and the start of 2026 the clock check uses.
  CHECK_EQ(epoch_seconds(2026, 1, 1, 0, 0, 0), (int64_t) 1767225600);
  CHECK_EQ(epoch_seconds(2026, 10, 7, 12, 34, 56), (int64_t) 1791376496);
  CHECK_EQ(epoch_seconds(2000, 3, 1, 0, 0, 0), (int64_t) 951868800);
  CHECK_EQ(epoch_seconds(2038, 1, 19, 3, 14, 8), (int64_t) 2147483648ll);
}

TEST(leap_days_are_counted_by_the_gregorian_rule) {
  // 2024 and 2000 are leap years; 2100 and 1900 are not.
  CHECK_EQ(days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28), (int64_t) 2);
  CHECK_EQ(days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28), (int64_t) 2);
  CHECK_EQ(days_from_civil(2100, 3, 1) - days_from_civil(2100, 2, 28), (int64_t) 1);
  CHECK_EQ(days_from_civil(1900, 3, 1) - days_from_civil(1900, 2, 28), (int64_t) 1);
}

TEST(a_year_is_365_or_366_days) {
  CHECK_EQ(days_from_civil(2027, 1, 1) - days_from_civil(2026, 1, 1), (int64_t) 365);
  CHECK_EQ(days_from_civil(2025, 1, 1) - days_from_civil(2024, 1, 1), (int64_t) 366);
}

TEST(dates_before_1970_are_negative) {
  CHECK_EQ(epoch_seconds(1969, 12, 31, 23, 59, 59), (int64_t) -1);
  CHECK(epoch_seconds(1900, 1, 1, 0, 0, 0) < 0);
}

TEST(a_ninety_day_certificate_ends_ninety_days_on) {
  CHECK_EQ(epoch_seconds(2026, 10, 8, 0, 0, 0) + 90 * 86400, epoch_seconds(2027, 1, 6, 0, 0, 0));
}
