#include "check.h"
#include "eink/core/battery.h"

using namespace eink_battery;

// The thresholds the YAML ships with.
constexpr float LOW = 3.40f, EMPTY = 3.30f, RESUME = 3.60f;

static Verdict judge_(float v, bool halted, bool low) { return judge(v, halted, low, LOW, EMPTY, RESUME); }

TEST(only_a_believable_voltage_counts) {
  CHECK(!plausible(0.0f));
  CHECK(!plausible(2.49f));
  CHECK(plausible(2.5f));
  CHECK(plausible(4.2f));
  CHECK(plausible(5.0f));
  CHECK(!plausible(5.01f));
  CHECK(!plausible(0.0f / 0.0f));  // NaN is never a reading
}

TEST(percent_follows_the_curve_at_each_knot) {
  CHECK_EQ(percent(3.27f), 0);
  CHECK_EQ(percent(3.30f), 5);
  CHECK_EQ(percent(3.41f), 10);
  CHECK_EQ(percent(3.75f), 50);
  CHECK_EQ(percent(3.96f), 90);
  CHECK_EQ(percent(4.15f), 100);
}

TEST(percent_is_held_at_the_ends_and_never_goes_backwards) {
  CHECK_EQ(percent(0.0f), 0);
  CHECK_EQ(percent(3.0f), 0);
  CHECK_EQ(percent(4.5f), 100);
  int last = 0;
  for (int mv = 3000; mv <= 4300; mv++) {
    const int pct = percent(mv / 1000.0f);
    CHECK(pct >= last && pct >= 0 && pct <= 100);
    last = pct;
  }
}

TEST(state_names) {
  CHECK_EQ(std::string(state_name(State::OK)), "ok");
  CHECK_EQ(std::string(state_name(State::LOW)), "low");
  CHECK_EQ(std::string(state_name(State::EMPTY)), "empty");
}

TEST(a_healthy_battery_carries_on) {
  Verdict v = judge_(3.9f, false, false);
  CHECK(!v.halted && !v.low);
  CHECK_EQ(v.state, State::OK);
  CHECK_EQ(v.action, Action::CARRY_ON);
}

TEST(going_low_warns_but_does_not_stop) {
  Verdict v = judge_(3.35f, false, false);
  CHECK(!v.halted && v.low);
  CHECK_EQ(v.state, State::LOW);
  CHECK_EQ(v.action, Action::CARRY_ON);
}

TEST(going_empty_halts_once_and_says_so_first) {
  Verdict first = judge_(3.2f, false, true);
  CHECK(first.halted && first.low);
  CHECK_EQ(first.state, State::EMPTY);
  CHECK_EQ(first.action, Action::HALT_NOW);  // halting now: show the notice
  Verdict again = judge_(3.2f, first.halted, first.low);
  CHECK_EQ(again.action, Action::STILL_HALTED);  // already halted: just sleep
}

TEST(the_latches_clear_only_above_the_resume_voltage_and_together) {
  // Recovering to between empty and resume is still halted: no flapping near the edge.
  CHECK(judge_(3.5f, true, true).halted);
  CHECK(judge_(3.59f, true, true).halted);
  CHECK(judge_(3.59f, true, true).low);
  Verdict up = judge_(3.6f, true, true);
  CHECK(!up.halted && !up.low);
  CHECK_EQ(up.state, State::OK);
  CHECK_EQ(up.action, Action::CARRY_ON);
}

TEST(low_stays_until_resume_even_when_the_voltage_rises_past_the_low_threshold) {
  CHECK(judge_(3.45f, false, true).low);
  CHECK(!judge_(3.65f, false, true).low);
}

TEST(the_thresholds_are_strict_below) {
  CHECK(!judge_(LOW, false, false).low);
  CHECK(judge_(3.399f, false, false).low);
  CHECK(!judge_(EMPTY, false, false).halted);
  CHECK(judge_(3.299f, false, false).halted);
}

TEST(millivolts_round_to_the_nearest) {
  CHECK_EQ(millivolts(3.7124f), 3712u);
  CHECK_EQ(millivolts(3.7126f), 3713u);
  CHECK_EQ(millivolts(4.2f), 4200u);
}

TEST(an_unusable_reading_is_not_known_and_changes_nothing) {
  for (float volts : {0.0f, 2.4f, 5.1f}) {
    Assessment a = assess(volts, true, true, LOW, EMPTY, RESUME);
    CHECK(!a.known);
    CHECK_EQ(a.verdict.action, Action::CARRY_ON);  // never a reason to stop
    CHECK(a.verdict.halted && a.verdict.low);      // the latches stay as they were
    Assessment b = assess(volts, false, false, LOW, EMPTY, RESUME);
    CHECK(!b.known && !b.verdict.halted && !b.verdict.low);
  }
}

TEST(a_usable_reading_gives_the_charge_the_telemetry_and_the_verdict) {
  Assessment a = assess(3.35f, false, false, LOW, EMPTY, RESUME);
  CHECK(a.known);
  CHECK_EQ(a.mv, 3350u);
  CHECK_EQ(a.pct, percent(3.35f));
  CHECK(a.verdict.low && !a.verdict.halted);
  CHECK_EQ(a.verdict.state, State::LOW);
  // The same verdict `judge` gives.
  Verdict v = judge_(3.35f, false, false);
  CHECK_EQ(a.verdict.action, v.action);
}
