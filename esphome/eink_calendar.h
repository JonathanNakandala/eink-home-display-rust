// Calendar arithmetic with no library behind it: the seconds since 1970 of a date and time in UTC. The chip has no
// timezone database and `timegm` is not portable, and a certificate's dates arrive as year, month, day and so on.
//
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
#pragma once

#include <cstdint>

namespace eink_calendar {

// Days from 1970-01-01 to the given day of the proleptic Gregorian calendar (Howard Hinnant's algorithm).
constexpr int64_t days_from_civil(int64_t y, int month, int day) {
  y -= month <= 2 ? 1 : 0;
  const int64_t era = (y >= 0 ? y : y - 399) / 400;
  const int64_t year_of_era = y - era * 400;
  const int64_t day_of_year = (153 * (month + (month > 2 ? -3 : 9)) + 2) / 5 + day - 1;
  const int64_t day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  return era * 146097 + day_of_era - 719468;
}

// Seconds since 1970-01-01 00:00:00 UTC.
constexpr int64_t epoch_seconds(int64_t year, int month, int day, int hour, int minute, int second) {
  return days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second;
}

}  // namespace eink_calendar
