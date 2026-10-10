#include <vector>

#include "check.h"
#include "home_display/core/hold.h"

using namespace home_display_hold;

// Follows a button the way the wake does: polled every 100 ms from `start`, down until `released_at` (or for ever).
// Returns each action that was not NONE, with the time it came.
struct Event {
  uint32_t at;
  Action action;
};

static std::vector<Event> hold(uint32_t start, uint32_t held_for, uint32_t until = 20000) {
  Tracker tracker;
  tracker.begin(start, true);
  std::vector<Event> events;
  // By the time since the start, so that the counter wrapping is the tracker's to get right and not this loop's.
  for (uint32_t since = 0; since <= until && tracker.active(); since += 100) {
    const Action a = tracker.update(start + since, since < held_for);
    if (a != Action::NONE)
      events.push_back({since, a});
  }
  return events;
}

TEST(a_button_not_held_at_the_start_is_not_followed_at_all) {
  Tracker tracker;
  tracker.begin(1000, false);
  CHECK(!tracker.active());
  CHECK(tracker.update(1100, true) == Action::NONE);  // pressed after the start: not this
}

TEST(a_press_that_ends_before_the_warning_resets_nothing_and_says_nothing) {
  for (uint32_t held : {50u, 300u, 1000u, 4900u}) {
    const auto events = hold(0, held);
    CHECK(events.empty());
  }
}

TEST(held_for_five_seconds_the_panel_is_told_to_say_so_once) {
  const auto events = hold(0, 7000);
  CHECK_EQ(events.size(), 2u);
  CHECK(events[0].action == Action::WARN);
  CHECK_EQ(events[0].at, WARN_MS);
  // Let go at seven seconds: the warning is on the panel and has to be taken off.
  CHECK(events[1].action == Action::CANCEL);
}

TEST(held_for_ten_seconds_the_pairing_is_reset_and_the_button_is_no_longer_followed) {
  Tracker tracker;
  tracker.begin(500, true);
  std::vector<Action> seen;
  for (uint32_t t = 500; t < 500 + 12000 && tracker.active(); t += 100) {
    const Action a = tracker.update(t, true);
    if (a != Action::NONE)
      seen.push_back(a);
  }
  CHECK_EQ(seen.size(), 2u);
  CHECK(seen[0] == Action::WARN);
  CHECK(seen[1] == Action::RESET);
  CHECK(!tracker.active());
  CHECK(tracker.update(20000, true) == Action::NONE);  // once is enough
}

TEST(a_release_just_before_the_reset_cancels_it) {
  const auto events = hold(0, RESET_MS - 200);
  CHECK_EQ(events.size(), 2u);
  CHECK(events[1].action == Action::CANCEL);
}

TEST(a_bounce_in_the_button_is_not_a_release) {
  Tracker tracker;
  tracker.begin(0, true);
  uint32_t t = 0;
  // Down, up for 100 ms (a bounce), down again, all the way to the reset.
  for (; t < 3000; t += 100)
    CHECK(tracker.update(t, true) == Action::NONE);
  CHECK(tracker.update(t, false) == Action::NONE);
  t += 100;
  CHECK(tracker.update(t, true) == Action::NONE);
  CHECK(tracker.active());
  bool reset = false;
  for (t += 100; t < 12000 && tracker.active(); t += 100)
    reset = reset || tracker.update(t, true) == Action::RESET;
  CHECK(reset);
}

TEST(a_poll_that_comes_late_still_gets_the_reset_not_just_the_warning) {
  Tracker tracker;
  tracker.begin(0, true);
  // The panel update for the warning takes seconds, so the next look at the button can be well past ten.
  CHECK(tracker.update(5000, true) == Action::WARN);
  CHECK(tracker.update(11500, true) == Action::RESET);
}

TEST(a_poll_that_jumps_over_both_times_resets_at_once) {
  Tracker tracker;
  tracker.begin(0, true);
  CHECK(tracker.update(12000, true) == Action::RESET);
}

TEST(the_times_are_right_across_the_counter_wrapping) {
  const uint32_t start = 0xFFFFFFFFu - 6000;  // the counter wraps six seconds in: after the warning, before the reset
  const auto events = hold(start, 30000);
  CHECK_EQ(events.size(), 2u);
  CHECK_EQ(events[0].at, WARN_MS);
  CHECK(events[1].action == Action::RESET);
  CHECK_EQ(events[1].at, RESET_MS);
}

TEST(a_release_after_the_warning_is_noticed_only_when_it_lasts) {
  Tracker tracker;
  tracker.begin(0, true);
  CHECK(tracker.update(5000, true) == Action::WARN);
  CHECK(tracker.update(5100, false) == Action::NONE);
  CHECK(tracker.update(5300, false) == Action::NONE);  // 200 ms up: not yet
  CHECK(tracker.update(5400, false) == Action::CANCEL);
  CHECK(!tracker.active());
}

TEST(what_the_panel_says_names_the_seconds_left_and_the_outcome) {
  CHECK_EQ(warning_notice(), "Keep holding to reset pairing (5 s)");
  CHECK_EQ(done_notice(true), "Pairing reset");
  CHECK_EQ(done_notice(false), "Could not reset pairing");
}
