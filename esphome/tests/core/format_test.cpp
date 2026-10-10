#include "check.h"
#include "home_display/core/format.h"

using namespace home_display_format;
using home_display_report::Failure;

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
  CHECK_EQ(clock_text(NOON_ISH, 3600), "13:34");                 // UK in summer
  CHECK_EQ(clock_text(NOON_ISH, -5 * 3600), "07:34");            // New York in winter
  CHECK_EQ(clock_text(NOON_ISH, 5 * 3600 + 1800), "18:04");      // Colombo, a half hour
  CHECK_EQ(clock_text(NOON_ISH, 10 * 3600 + 1800), "23:04");     // Lord Howe in winter
  CHECK_EQ(clock_text(NOON_ISH, 12 * 3600 + 45 * 60), "01:19");  // Chatham, 45 minutes, into the next day
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

TEST(the_server_is_where_mdns_found_it_or_the_fallback) {
  const uint32_t ip = 192u | (168u << 8) | (1u << 16) | (20u << 24);
  const std::string fallback = "http://home-display.local:8080";
  CHECK_EQ(server_base(ip, 8080, fallback), "http://192.168.1.20:8080");
  CHECK_EQ(server_base(0, 8080, fallback), fallback);  // no address
  CHECK_EQ(server_base(ip, 0, fallback), fallback);    // no port
  CHECK_EQ(server_base(0, 0, fallback), fallback);
}

TEST(a_failure_notice_names_the_cause_and_the_time_only_when_it_is_known) {
  const int64_t now = 1791376496;  // 12:34:56 UTC; an hour ahead in the server's zone
  CHECK_EQ(failure_notice(Failure::SERVER, true, now, true, 3600), "Last update failed @ 13:34");
  CHECK_EQ(failure_notice(Failure::WIFI, true, now, true, 3600), "No Wi-Fi @ 13:34");
  CHECK_EQ(failure_notice(Failure::MEMORY, true, now, true, 0), "Out of memory @ 12:34");
  // No time is better than a wrong one.
  CHECK_EQ(failure_notice(Failure::SERVER, false, now, true, 3600), "Last update failed");  // clock not set
  CHECK_EQ(failure_notice(Failure::SERVER, true, now, false, 0), "Last update failed");     // no plan ever answered
  CHECK_EQ(failure_notice(Failure::WIFI, false, 0, false, 0), "No Wi-Fi");
  CHECK_EQ(failure_notice(Failure::NONE, true, now, true, 0), "Last update failed @ 12:34");
  CHECK_EQ(failure_notice(Failure::DOWNLOAD, true, now, true, 0), "Last update failed @ 12:34");
  CHECK_EQ(failure_notice(Failure::TIMEOUT, true, now, true, 0), "Last update failed @ 12:34");
  // What the secure transport reports, each in its own words.
  CHECK_EQ(failure_notice(Failure::CLOCK, false, 0, false, 0), "Clock not set");
  CHECK_EQ(failure_notice(Failure::CERTIFICATE, true, now, true, 0), "Certificate refused @ 12:34");
  CHECK_EQ(failure_notice(Failure::APPROVAL, true, now, true, 3600), "Waiting for approval @ 13:34");
  CHECK_EQ(failure_notice(Failure::UNRECOGNISED, false, 0, false, 0), "Not recognised - ask the owner to approve");
}

TEST(the_stale_notice_says_how_old_the_image_is) {
  CHECK_EQ(stale_notice(0), "Out of date: rendered under 1 min ago");
  CHECK_EQ(stale_notice(3 * 3600 + 20 * 60), "Out of date: rendered 3 h 20 min ago");
}

// The font on the panel has only these glyphs (packages/display.yaml, `glyphs:`); a character outside them would be
// drawn as nothing, so a notice must be made only of them.
static const std::string NOTICE_GLYPHS = " :@%-.0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

TEST(every_notice_is_made_only_of_glyphs_the_font_has) {
  using home_display_report::LAST_FAILURE;
  const int64_t now = 1791376496;
  for (uint8_t value = 0; value <= static_cast<uint8_t>(LAST_FAILURE); value++) {
    for (bool known : {true, false}) {
      const std::string text = failure_notice(static_cast<Failure>(value), known, now, known, 3600);
      CHECK(text.find_first_not_of(NOTICE_GLYPHS) == std::string::npos);
    }
  }
  CHECK(stale_notice(3 * 3600 + 20 * 60).find_first_not_of(NOTICE_GLYPHS) == std::string::npos);
  CHECK(std::string("Battery empty - charge to resume").find_first_not_of(NOTICE_GLYPHS) == std::string::npos);
}
