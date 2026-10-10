#include <array>
#include <string>
#include <vector>

#include "check.h"
#include "home_display/core/report.h"

using namespace home_display_report;

static const std::array<Failure, 9> ALL_FAILURES = {Failure::WIFI,        Failure::SERVER,   Failure::DOWNLOAD,
                                                    Failure::MEMORY,      Failure::TIMEOUT,  Failure::CLOCK,
                                                    Failure::CERTIFICATE, Failure::APPROVAL, Failure::UNRECOGNISED};

static const Battery GOOD = {true, 3712, 47, "ok"};
static const Battery NONE_KNOWN = {false, 0, 0, "ok"};

TEST(a_first_report_has_only_what_is_known) {
  CHECK_EQ(query("kitchen", 0, NONE_KNOWN, 0, EMPTY), "&device=kitchen&failed_wakes=0");
}

// The same string is parsed by the server in src/adapters/image_server/plan.rs
// (`the_exact_report_the_firmware_builds_is_understood`). Change one and the other test fails.
TEST(a_full_report_has_every_field_in_a_fixed_order) {
  Last last = EMPTY;
  set_failure(last, Failure::DOWNLOAD);
  set_wake(last, 24400);
  set_connection(last, 1100, 61440);
  CHECK_EQ(query("reterminal-e1003-a1b2c3", 2, GOOD, -67, last, "0.1.0"),
           "&device=reterminal-e1003-a1b2c3&failed_wakes=2&battery_mv=3712&battery_pct=47&battery_state=ok"
           "&rssi=-67&last_failure=download&last_wake_s=24&last_tls_ms=1100&last_heap_min=61440&fw=0.1.0");
}

TEST(the_firmware_version_is_told_only_when_there_is_one_and_only_in_characters_a_query_keeps) {
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, EMPTY).find("fw="), std::string::npos);
  CHECK_EQ(firmware_token("0.2.0"), "0.2.0");
  CHECK_EQ(firmware_token("0.3.0-beta.1"), "0.3.0-beta.1");
  CHECK_EQ(firmware_token("1.0.0+build 5&x=y"), "1.0.0_build_5_x_y");  // nothing that would end the value or add a key
  CHECK_EQ(firmware_token(std::string(100, '1')).size(), MAX_FIRMWARE_CHARS);
  CHECK(query("a", 0, NONE_KNOWN, 0, EMPTY, "1.0.0+x").find("&fw=1.0.0_x") != std::string::npos);
}

TEST(the_handshake_time_and_the_heap_are_told_only_when_they_were_measured) {
  Last last = EMPTY;
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, last).find("last_tls_ms"), std::string::npos);
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, last).find("last_heap_min"), std::string::npos);
  set_connection(last, 0, 50000);  // a wake with no TLS: no time, but the heap was read
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, last).find("last_tls_ms"), std::string::npos);
  CHECK(query("a", 0, NONE_KNOWN, 0, last).find("&last_heap_min=50000") != std::string::npos);
  // A wake that had none after a wake that had one: the earlier figure is not told again.
  set_connection(last, 900, 0);
  set_connection(last, 0, 0);
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, last).find("last_tls_ms"), std::string::npos);
}

TEST(a_figure_past_what_the_server_believes_is_held_at_its_limit) {
  Last last = EMPTY;
  set_connection(last, 5000000, 0xFFFFFFFFu);
  CHECK_EQ(last.tls_ms, MAX_TLS_MS);
  CHECK_EQ(last.heap_min_bytes, MAX_HEAP_BYTES);
}

TEST(a_signal_that_is_not_a_reading_is_left_out) {
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, EMPTY).find("rssi"), std::string::npos);
  CHECK_EQ(query("a", 0, NONE_KNOWN, 5, EMPTY).find("rssi"), std::string::npos);
  CHECK(query("a", 0, NONE_KNOWN, -1, EMPTY).find("&rssi=-1") != std::string::npos);
}

TEST(a_battery_that_could_not_be_read_is_left_out) {
  CHECK_EQ(query("a", 1, NONE_KNOWN, 0, EMPTY).find("battery"), std::string::npos);
}

TEST(every_failure_is_set_and_reported_and_none_is_absent) {
  for (Failure failure : ALL_FAILURES) {
    Last last = EMPTY;
    set_failure(last, failure);
    CHECK(query("a", 1, NONE_KNOWN, 0, last).find(std::string("&last_failure=") + failure_name(failure)) !=
          std::string::npos);
  }
  CHECK_EQ(query("a", 0, NONE_KNOWN, 0, EMPTY).find("last_failure"), std::string::npos);
}

TEST(every_failure_has_its_own_name_and_nothing_beyond_the_last_does) {
  std::vector<std::string> seen;
  for (uint8_t value = 1; value <= static_cast<uint8_t>(LAST_FAILURE); value++) {
    const char *name = failure_name(static_cast<Failure>(value));
    CHECK(name != nullptr);
    if (name == nullptr)
      continue;
    for (const std::string &other : seen)
      CHECK(other != name);
    seen.push_back(name);
  }
  CHECK_EQ(seen.size(), ALL_FAILURES.size());
  CHECK(failure_name(static_cast<Failure>(static_cast<uint8_t>(LAST_FAILURE) + 1)) == nullptr);
}

TEST(a_failure_the_log_can_always_name) {
  CHECK_EQ(std::string(failure_label(Failure::NONE)), "none");
  CHECK_EQ(std::string(failure_label(Failure::TIMEOUT)), "timeout");
  CHECK_EQ(std::string(failure_label(static_cast<Failure>(200))), "none");
}

TEST(the_failure_names_match_what_the_server_understands) {
  // src/application/devices.rs, FailureReason::as_str: the same nine, pinned there too.
  CHECK_EQ(std::string(failure_name(Failure::WIFI)), "wifi");
  CHECK_EQ(std::string(failure_name(Failure::SERVER)), "server");
  CHECK_EQ(std::string(failure_name(Failure::DOWNLOAD)), "download");
  CHECK_EQ(std::string(failure_name(Failure::MEMORY)), "memory");
  CHECK_EQ(std::string(failure_name(Failure::TIMEOUT)), "timeout");
  CHECK_EQ(std::string(failure_name(Failure::CLOCK)), "clock");
  CHECK_EQ(std::string(failure_name(Failure::CERTIFICATE)), "certificate");
  CHECK_EQ(std::string(failure_name(Failure::APPROVAL)), "approval");
  CHECK_EQ(std::string(failure_name(Failure::UNRECOGNISED)), "unrecognised");
  CHECK(failure_name(Failure::NONE) == nullptr);
  CHECK(failure_name(static_cast<Failure>(200)) == nullptr);
}

TEST(a_wake_time_is_rounded_and_capped_at_what_the_server_believes) {
  Last last = EMPTY;
  set_wake(last, 0);
  CHECK_EQ(last.wake_seconds, (uint16_t) 0);
  CHECK(last.wake_known);
  set_wake(last, 24499);
  CHECK_EQ(last.wake_seconds, (uint16_t) 24);
  set_wake(last, 24500);
  CHECK_EQ(last.wake_seconds, (uint16_t) 25);
  set_wake(last, 600000);
  CHECK_EQ(last.wake_seconds, (uint16_t) 600);
  set_wake(last, 4000000000u);  // no overflow of the 16-bit field
  CHECK_EQ(last.wake_seconds, (uint16_t) 600);
}

TEST(memory_left_by_another_firmware_is_not_believed) {
  CHECK(valid(EMPTY));
  Last stale = EMPTY;
  stale.magic = 0;
  CHECK(!valid(stale));
  Last odd = EMPTY;
  odd.failure = static_cast<Failure>(99);
  CHECK(!valid(odd));
}
