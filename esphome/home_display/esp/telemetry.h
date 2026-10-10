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
#include "esp_heap_caps.h"
#include "esphome/components/wifi/wifi_component.h"
#include "esphome/core/defines.h"

#include "home_display/core/report.h"

namespace home_display_telemetry {

// Inline, so that it is one object whatever includes this header (see esp/secure_chip.h).
inline RTC_DATA_ATTR home_display_report::Last last = home_display_report::EMPTY;

inline home_display_report::Last &get() {
  if (!home_display_report::valid(last))
    last = home_display_report::EMPTY;
  return last;
}

inline void set_failure(home_display_report::Failure reason) { home_display_report::set_failure(get(), reason); }

inline void clear_failure() { get().failure = home_display_report::Failure::NONE; }

// Called just before sleeping, with the time since boot, which is how long this wake was awake, and how long the wake's
// first TLS handshake took (0 if there was none). The least free heap of the wake is read here: the lowest it has been
// since boot, and a wake starts at boot, so the handshake is in it. Internal memory only, which is what mbedTLS runs
// out of; the picture's buffer is in PSRAM.
inline void record_wake(uint32_t awake_ms, uint32_t tls_ms = 0) {
  home_display_report::set_wake(get(), awake_ms);
  home_display_report::set_connection(
      get(), tls_ms, static_cast<uint32_t>(heap_caps_get_minimum_free_size(MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT)));
}

// Wi-Fi signal in dBm, or 0 when there is no reading (the server ignores 0).
inline int rssi() {
  auto *wifi = esphome::wifi::global_wifi_component;
  return wifi == nullptr ? 0 : (int) wifi->wifi_rssi();
}

// The version of the firmware: the release it was built from (packages/version.yaml), compiled in by `esphome:
// project:`.
inline const char *firmware_version() {
#ifdef ESPHOME_PROJECT_VERSION
  return ESPHOME_PROJECT_VERSION;
#else
  return "";
#endif
}

inline std::string query(const std::string &device, unsigned failed_wakes,
                         const home_display_report::Battery &battery) {
  return home_display_report::query(device, failed_wakes, battery, rssi(), get(), firmware_version());
}

}  // namespace home_display_telemetry
