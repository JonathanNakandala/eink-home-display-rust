// What the display tells the server about itself on each check-in, as the query-string tail of its /plan and
// /refresh requests (the server reads it in src/application/devices.rs, which also decides what is believable).
//
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
// Where the values come from (the radio, RTC memory) is eink_telemetry.h.
#pragma once

#include <cstdint>
#include <string>

namespace eink_report {

// Why the last failed wake failed. The names are what `last_failure` is sent as.
enum Failure : uint8_t { NONE = 0, WIFI = 1, SERVER = 2, DOWNLOAD = 3, MEMORY = 4, TIMEOUT = 5 };

// The longest wake that is reported; the server drops anything above its own limit of the same size.
constexpr uint32_t MAX_WAKE_SECONDS = 600;

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

// What is remembered about the last wake and told to the server by the next one.
struct Last {
  uint32_t magic;
  uint8_t failure;
  uint16_t wake_seconds;
  bool wake_known;
};

constexpr uint32_t MAGIC = 0xE1B70001;
constexpr Last EMPTY = {MAGIC, NONE, 0, false};

// Memory that survives a software reset (an update over the air) may hold a layout from another firmware, so
// it is checked before it is believed.
inline bool valid(const Last &last) { return last.magic == MAGIC && last.failure <= TIMEOUT; }

// A failed wake, by the reason the YAML names it (wifi, server, download, memory or timeout). Anything else
// leaves what was there.
inline void set_failure(Last &last, const std::string &reason) {
  for (uint8_t failure = WIFI; failure <= TIMEOUT; failure++)
    if (reason == failure_name(failure))
      last.failure = failure;
}

// A wake that was awake for `awake_ms`: rounded to seconds, and no more than the server will believe.
inline void set_wake(Last &last, uint32_t awake_ms) {
  const uint32_t seconds = awake_ms / 1000 + (awake_ms % 1000 >= 500 ? 1 : 0);
  last.wake_seconds = seconds > MAX_WAKE_SECONDS ? MAX_WAKE_SECONDS : seconds;
  last.wake_known = true;
}

struct Battery {
  bool known;  // false when it could not be read, so the server doesn't record a made-up value
  uint32_t millivolts;
  int percent;
  const char *state;
};

// The query-string tail: who this is, how it is doing, and how the last wake went. `rssi_dbm` is left out when
// it is not a reading (0 or more); the last wake's figures are left out until there has been one.
inline std::string query(const std::string &device, unsigned failed_wakes, const Battery &battery, int rssi_dbm,
                         const Last &last) {
  std::string out = "&device=" + device + "&failed_wakes=" + std::to_string(failed_wakes);
  if (battery.known) {
    out += "&battery_mv=" + std::to_string(battery.millivolts) + "&battery_pct=" + std::to_string(battery.percent) +
           "&battery_state=" + battery.state;
  }
  if (rssi_dbm < 0)
    out += "&rssi=" + std::to_string(rssi_dbm);
  if (const char *reason = failure_name(last.failure))
    out += std::string("&last_failure=") + reason;
  if (last.wake_known)
    out += "&last_wake_s=" + std::to_string(last.wake_seconds);
  return out;
}

}  // namespace eink_report
