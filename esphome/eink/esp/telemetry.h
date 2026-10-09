// The device side of the check-in report: where the values come from. The arithmetic and the wording are in
// core/report.h, which is tested on a computer; this is only the radio and RTC memory, so it can't be.
//
// A wake asks /plan before it knows how it will go, so a failure can only be reported by the *next* wake that
// gets through, and the wake time is that of the one before. Those two are kept in RTC memory, which stays
// powered through deep sleep: no flash is written, and a power loss simply forgets them, which is fine for
// diagnostics. Everything the display must not lose (the shown version, the battery latches, the failure
// count) is kept in flash by the YAML's `restore_value` globals instead.
#pragma once

#include <cstdint>
#include <string>

#include "esp_attr.h"
#include "esphome/components/wifi/wifi_component.h"

#include "eink/core/report.h"

namespace eink_telemetry {

static RTC_DATA_ATTR eink_report::Last last = eink_report::EMPTY;

inline eink_report::Last &get() {
  if (!eink_report::valid(last))
    last = eink_report::EMPTY;
  return last;
}

inline void set_failure(eink_report::Failure reason) { eink_report::set_failure(get(), reason); }

inline void clear_failure() { get().failure = eink_report::Failure::NONE; }

// Called just before sleeping, with the time since boot, which is how long this wake was awake.
inline void record_wake(uint32_t awake_ms) { eink_report::set_wake(get(), awake_ms); }

// Wi-Fi signal in dBm, or 0 when there is no reading (the server ignores 0).
inline int rssi() {
  auto *wifi = esphome::wifi::global_wifi_component;
  return wifi == nullptr ? 0 : (int) wifi->wifi_rssi();
}

inline std::string query(const std::string &device, unsigned failed_wakes, const eink_report::Battery &battery) {
  return eink_report::query(device, failed_wakes, battery, rssi(), get());
}

}  // namespace eink_telemetry
