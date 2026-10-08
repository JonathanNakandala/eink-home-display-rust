// Battery arithmetic: charge from voltage, and the latches that decide when to warn and when to stop.
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
// Reading the voltage is the YAML's job.
#pragma once

#include <cstdint>

namespace eink_battery {

// Plausible range for a Li-ion cell behind the board's divider. Outside it the reading is a
// missing battery or an ADC fault, not a charge level, and must never trigger a shutdown.
constexpr float MIN_PLAUSIBLE_V = 2.5f;
constexpr float MAX_PLAUSIBLE_V = 5.0f;

inline bool plausible(float volts) { return volts >= MIN_PLAUSIBLE_V && volts <= MAX_PLAUSIBLE_V; }

// Charge in percent, interpolated from the discharge curve Seeed's example uses.
inline int percent(float v) {
  static const float curve[][2] = {{3.27f, 0},  {3.30f, 5},  {3.41f, 10}, {3.49f, 20}, {3.58f, 30}, {3.68f, 40},
                                   {3.75f, 50}, {3.80f, 60}, {3.85f, 70}, {3.91f, 80}, {3.96f, 90}, {4.15f, 100}};
  constexpr int n = sizeof(curve) / sizeof(curve[0]);
  if (v <= curve[0][0])
    return 0;
  if (v >= curve[n - 1][0])
    return 100;
  for (int i = 1; i < n; i++) {
    if (v <= curve[i][0]) {
      float t = (v - curve[i - 1][0]) / (curve[i][0] - curve[i - 1][0]);
      return (int) (curve[i - 1][1] + t * (curve[i][1] - curve[i - 1][1]) + 0.5f);
    }
  }
  return 100;
}

inline const char *state_name(int state) { return state == 2 ? "empty" : state == 1 ? "low" : "ok"; }

// What this wake's reading means, given the latches from the last one. Each latch sets below its own
// threshold and clears only above `resume_v`, together, so neither flaps near its edge.
struct Verdict {
  bool halted;
  bool low;
  int state;   // 0 ok, 1 low, 2 empty
  int action;  // 0 carry on, 1 still halted (just sleep), 2 halting now (say so first)
};

inline Verdict judge(float volts, bool was_halted, bool was_low, float low_v, float empty_v, float resume_v) {
  bool halted = was_halted, low = was_low;
  if (volts >= resume_v) {
    halted = false;
    low = false;
  } else {
    if (volts < empty_v)
      halted = true;
    if (volts < low_v)
      low = true;
  }
  return Verdict{halted, low, halted ? 2 : low ? 1 : 0, halted ? (was_halted ? 1 : 2) : 0};
}

// Millivolts, rounded, for the telemetry.
inline uint32_t millivolts(float volts) { return (uint32_t) (volts * 1000.0f + 0.5f); }

// Everything a wake takes from one reading. `known` is false for a missing battery or a bad reading, which is
// never a reason to stop: then only `known` means anything, and the latches are left as they were.
struct Assessment {
  bool known;
  uint32_t mv;
  int pct;
  Verdict verdict;
};

inline Assessment assess(float volts, bool was_halted, bool was_low, float low_v, float empty_v, float resume_v) {
  if (!plausible(volts))
    return Assessment{false, 0, 0, Verdict{was_halted, was_low, 0, 0}};
  return Assessment{true, millivolts(volts), percent(volts),
                    judge(volts, was_halted, was_low, low_v, empty_v, resume_v)};
}

}  // namespace eink_battery
