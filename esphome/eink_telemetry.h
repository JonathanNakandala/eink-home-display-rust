// What the display tells the server about itself on each check-in, as the query-string tail of its
// /plan and /refresh requests (the server reads it in src/application/devices.rs).
//
// A wake asks /plan before it knows how it will go, so a failure can only be reported by the *next*
// wake that gets through, and the wake time is that of the one before. Those two are kept in RTC memory,
// which stays powered through deep sleep: no flash is written, and a power loss simply forgets them,
// which is fine for diagnostics. Everything the display must not lose (the shown version, the battery
// latches, the failure count) is kept in flash by the YAML's `restore_value` globals instead.
#pragma once

#include <cstdint>
#include <string>

#include "esp_attr.h"
#include "esphome/components/wifi/wifi_component.h"

namespace eink_telemetry {

// Why the last failed wake failed. The names are what `last_failure` is sent as.
enum Failure : uint8_t { NONE = 0, WIFI = 1, SERVER = 2, DOWNLOAD = 3, MEMORY = 4, TIMEOUT = 5 };

inline const char *failure_name(uint8_t failure) {
  switch (failure) {
    case WIFI: return "wifi";
    case SERVER: return "server";
    case DOWNLOAD: return "download";
    case MEMORY: return "memory";
    case TIMEOUT: return "timeout";
    default: return nullptr;
  }
}

// The longest wake that is reported; the server drops anything above its own limit of the same size.
constexpr uint32_t MAX_WAKE_SECONDS = 600;

namespace detail {

constexpr uint32_t MAGIC = 0xE1B70001;

struct State {
  uint32_t magic;
  uint8_t last_failure;
  uint16_t last_wake_seconds;
  bool wake_known;
};

// Set only on a cold boot. After a software reset, such as an update over the air, RTC memory keeps whatever
// the previous firmware left there, which may not be this layout, so it is checked before it is believed.
static RTC_DATA_ATTR State state = {MAGIC, NONE, 0, false};

inline State &get() {
  if (state.magic != MAGIC || state.last_failure > TIMEOUT)
    state = State{MAGIC, NONE, 0, false};
  return state;
}

}  // namespace detail

// A failed wake, by the reason the YAML names it (wifi, server, download, memory or timeout).
inline void set_failure(const std::string &reason) {
  for (uint8_t failure = WIFI; failure <= TIMEOUT; failure++)
    if (reason == failure_name(failure))
      detail::get().last_failure = failure;
}

// The wake worked.
inline void clear_failure() { detail::get().last_failure = NONE; }

// Called just before sleeping, with the time since boot, which is how long this wake was awake.
inline void record_wake(uint32_t awake_ms) {
  detail::State &state = detail::get();
  const uint32_t seconds = (awake_ms + 500) / 1000;
  state.last_wake_seconds = seconds > MAX_WAKE_SECONDS ? MAX_WAKE_SECONDS : seconds;
  state.wake_known = true;
}

// Wi-Fi signal in dBm, or 0 when there is no reading (the server ignores 0).
inline int rssi() {
  auto *wifi = esphome::wifi::global_wifi_component;
  return wifi == nullptr ? 0 : (int) wifi->wifi_rssi();
}

// The query-string tail: who this is, how it is doing, and how the last wake went. The battery is left out
// when it could not be read, so the server doesn't record a made-up value; the last wake's figures are left
// out until there has been one.
inline std::string query(const char *device, unsigned failed_wakes, bool battery_known, uint32_t battery_mv,
                         int battery_pct, const char *battery_state) {
  std::string out = std::string("&device=") + device + "&failed_wakes=" + std::to_string(failed_wakes);
  if (battery_known) {
    out += "&battery_mv=" + std::to_string(battery_mv) + "&battery_pct=" + std::to_string(battery_pct) +
           "&battery_state=" + battery_state;
  }
  const int dbm = rssi();
  if (dbm < 0)
    out += "&rssi=" + std::to_string(dbm);
  const detail::State &state = detail::get();
  if (const char *reason = failure_name(state.last_failure))
    out += std::string("&last_failure=") + reason;
  if (state.wake_known)
    out += "&last_wake_s=" + std::to_string(state.last_wake_seconds);
  return out;
}

}  // namespace eink_telemetry
