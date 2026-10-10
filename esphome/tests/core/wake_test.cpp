#include <cstdint>

#include "check.h"
#include "home_display/core/wake.h"

using namespace home_display_wake;

TEST(plan_is_held_between_a_minute_and_a_day) {
  CHECK_EQ(plan_sleep_ms(0), 60000u);
  CHECK_EQ(plan_sleep_ms(59), 60000u);
  CHECK_EQ(plan_sleep_ms(60), 60000u);
  CHECK_EQ(plan_sleep_ms(600), 600000u);
  CHECK_EQ(plan_sleep_ms(86400), 86400000u);
  CHECK_EQ(plan_sleep_ms(86401), 86400000u);
  CHECK_EQ(plan_sleep_ms(UINT32_MAX), 86400000u);  // no wrap on a huge value
}

TEST(failures_count_up_and_stop_at_the_top) {
  CHECK_EQ(count_failure(0), (uint8_t) 1);
  CHECK_EQ(count_failure(254), (uint8_t) 255);
  CHECK_EQ(count_failure(255), (uint8_t) 255);  // never wraps back to "first failure"
}

TEST(backoff_doubles_up_to_the_cap) {
  const uint32_t base = 600000, cap = 3600000;  // 10 min, 1 h
  CHECK_EQ(backoff_ms(1, base, cap), 600000u);
  CHECK_EQ(backoff_ms(2, base, cap), 1200000u);
  CHECK_EQ(backoff_ms(3, base, cap), 2400000u);
  CHECK_EQ(backoff_ms(4, base, cap), 3600000u);  // 4.8 min doubled is over the cap
  CHECK_EQ(backoff_ms(8, base, cap), 3600000u);
  CHECK_EQ(backoff_ms(255, base, cap), 3600000u);
}

TEST(backoff_stops_doubling_after_six_when_the_cap_allows_more) {
  const uint32_t base = 1000, cap = 1000000;
  CHECK_EQ(backoff_ms(7, base, cap), 64000u);  // base << 6
  CHECK_EQ(backoff_ms(8, base, cap), 64000u);  // and no further
  CHECK_EQ(backoff_ms(255, base, cap), 64000u);
  // Landing exactly on the cap is the cap, not one under it.
  CHECK_EQ(backoff_ms(4, base, 8000), 8000u);
  CHECK_EQ(backoff_ms(4, base, 7999), 7999u);
  CHECK_EQ(backoff_ms(4, base, 8001), 8000u);
}

TEST(backoff_never_overflows_or_exceeds_the_cap) {
  // A base big enough that shifting it by six would not fit in 32 bits.
  CHECK_EQ(backoff_ms(255, 0xFFFFFFFFu, 3600000), 3600000u);
  CHECK_EQ(backoff_ms(8, 0xFFFFFFFFu, 0xFFFFFFFFu), 0xFFFFFFFFu);
  for (unsigned failed = 0; failed <= 255; failed++) {
    const uint32_t ms = backoff_ms((uint8_t) failed, 600000, 3600000);
    CHECK(ms >= 600000u && ms <= 3600000u);
  }
  CHECK_EQ(backoff_ms(0, 600000, 3600000), 600000u);  // before any failure is counted: no shift, no underflow
}

TEST(a_plan_sleep_is_shortened_by_what_the_wake_used) {
  // Asked to sleep 10 min from t=5000; it is now t=25000 (20 s used).
  CHECK_EQ(sleep_after(600000, 5000, 25000), 580000u);
  // Nothing used yet.
  CHECK_EQ(sleep_after(600000, 5000, 5000), 600000u);
}

TEST(a_sleep_that_is_not_the_plan_is_used_as_it_is) { CHECK_EQ(sleep_after(1200000, 0, 99999), 1200000u); }

TEST(a_wake_that_used_more_than_the_plan_still_sleeps_a_minute) {
  CHECK_EQ(sleep_after(600000, 1000, 1000 + 600000), 60000u);  // used all of it
  CHECK_EQ(sleep_after(600000, 1000, 1000 + 700000), 60000u);  // used more: no unsigned wrap to a huge sleep
  CHECK_EQ(sleep_after(600000, 1000, 1000 + 540000), 60000u);  // exactly a minute left
  CHECK_EQ(sleep_after(600000, 1000, 1000 + 540001), 60000u);  // just under: held at a minute
  CHECK_EQ(sleep_after(600000, 1000, 1000 + 539999), 60001u);  // just over
  CHECK_EQ(sleep_after(30000, 1000, 1500), 60000u);            // a plan already under a minute
}

TEST(the_clock_wrapping_does_not_upset_the_sum) {
  // received 10 s before the counter wraps, now 5 s after: 15 s used.
  const uint32_t received = 0xFFFFFFFFu - 9999u, now = 5000u;
  CHECK_EQ(sleep_after(600000, received, now), 585000u);
}

TEST(sleep_after_agrees_with_a_wide_reference_everywhere) {
  // The same sum in 64 bits, with no wrapping to go wrong, over a spread of values.
  for (uint64_t asked : {0ull, 59999ull, 60000ull, 600000ull, 86400000ull})
    for (uint64_t used :
         {0ull, 1ull, 59999ull, 60000ull, 540000ull, 540001ull, 600000ull, 86400000ull, 4000000000ull}) {
      const uint32_t received = 123456u;
      const uint32_t now = (uint32_t) (received + used);  // wraps for the big ones
      const uint64_t expected = asked > used + 60000 ? asked - used : 60000;
      CHECK_EQ((uint64_t) sleep_after((uint32_t) asked, received, now), expected);
    }
}

TEST(the_image_needs_a_byte_a_pixel_and_a_margin) {
  CHECK_EQ(image_bytes_needed(1872, 1404, 96 * 1024), (size_t) 1872 * 1404 + 96 * 1024);
  CHECK_EQ(image_bytes_needed(0, 0, 5), (size_t) 5);
}
