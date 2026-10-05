// Battery helpers: charge from voltage, and the telemetry the server records.
#pragma once

#include <cstdint>
#include <string>

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

// The query-string tail the server reads on /plan and /refresh: who this is, and how it is doing.
// The battery is left out when it could not be read, so the server doesn't record a made-up value.
inline std::string telemetry(const char *device, bool battery_known, uint32_t millivolts, int percent_value, int state,
                             unsigned failed_wakes) {
  std::string out = std::string("&device=") + device + "&failed_wakes=" + std::to_string(failed_wakes);
  if (battery_known) {
    out += "&battery_mv=" + std::to_string(millivolts) + "&battery_pct=" + std::to_string(percent_value) +
           "&battery_state=" + state_name(state);
  }
  return out;
}

}  // namespace eink_battery
