#include "check.h"
#include "../eink_format.h"

using namespace eink_format;

TEST(the_ip_is_read_first_octet_in_the_lowest_byte) {
  // 192.168.1.20, as lwIP stores it.
  const uint32_t ip = 192u | (168u << 8) | (1u << 16) | (20u << 24);
  CHECK_EQ(url(ip, 8080, ""), "http://192.168.1.20:8080");
  CHECK_EQ(url(ip, 8080, "/image"), "http://192.168.1.20:8080/image");
}

TEST(the_edges_of_an_address_and_port) {
  CHECK_EQ(url(0, 0, ""), "http://0.0.0.0:0");
  CHECK_EQ(url(0xFFFFFFFFu, 65535, "/plan"), "http://255.255.255.255:65535/plan");
}

TEST(an_age_is_in_the_largest_sensible_unit) {
  CHECK_EQ(age(0), "under 1 min");
  CHECK_EQ(age(59), "under 1 min");
  CHECK_EQ(age(60), "1 min");
  CHECK_EQ(age(59 * 60 + 59), "59 min");
  CHECK_EQ(age(3600), "1 h 0 min");
  CHECK_EQ(age(3 * 3600 + 20 * 60), "3 h 20 min");
  CHECK_EQ(age(26 * 3600), "26 h 0 min");
}

// 2026-10-07 12:34:56 UTC.
static const int64_t NOON_ISH = 1791376496;

TEST(the_time_of_day_is_utc_plus_the_offset) {
  CHECK_EQ(clock_text(NOON_ISH, 0), "12:34");
  CHECK_EQ(clock_text(NOON_ISH, 3600), "13:34");                  // UK in summer
  CHECK_EQ(clock_text(NOON_ISH, -5 * 3600), "07:34");             // New York in winter
  CHECK_EQ(clock_text(NOON_ISH, 5 * 3600 + 1800), "18:04");       // Colombo, a half hour
  CHECK_EQ(clock_text(NOON_ISH, 10 * 3600 + 1800), "23:04");      // Lord Howe in winter
  CHECK_EQ(clock_text(NOON_ISH, 12 * 3600 + 45 * 60), "01:19");   // Chatham, 45 minutes, into the next day
}

TEST(the_time_of_day_wraps_the_day_both_ways) {
  CHECK_EQ(clock_text(23 * 3600 + 30 * 60, 3600), "00:30");
  CHECK_EQ(clock_text(30 * 60, -3600), "23:30");
  CHECK_EQ(clock_text(0, 0), "00:00");
  CHECK_EQ(clock_text(86399, 0), "23:59");
  CHECK_EQ(clock_text(86400, 0), "00:00");
  CHECK_EQ(clock_text(-1, 0), "23:59");  // before 1970 still reads as a time of day
}

TEST(only_a_real_zone_offset_is_accepted) {
  CHECK(valid_utc_offset(0));
  CHECK(valid_utc_offset(-12 * 3600));
  CHECK(valid_utc_offset(14 * 3600));
  CHECK(valid_utc_offset(5 * 3600 + 1800));
  CHECK(!valid_utc_offset(-12 * 3600 - 1));
  CHECK(!valid_utc_offset(14 * 3600 + 1));
  CHECK(!valid_utc_offset(INT32_MAX));
  CHECK(!valid_utc_offset(INT64_MIN));
}
