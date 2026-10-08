// Text the device builds: a server's URL from what mDNS found, an age in words, and the time of day.
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
#pragma once

#include <cstdint>
#include <cstdio>
#include <string>

#include "eink_report.h"

namespace eink_format {

// `ip` is IPv4 in the byte order lwIP stores it (first octet in the lowest byte).
inline std::string url(uint32_t ip, uint16_t port, const std::string &path) {
  char buffer[64];
  snprintf(buffer, sizeof buffer, "http://%u.%u.%u.%u:%u", (unsigned) (ip & 0xff), (unsigned) ((ip >> 8) & 0xff),
           (unsigned) ((ip >> 16) & 0xff), (unsigned) ((ip >> 24) & 0xff), (unsigned) port);
  return buffer + path;
}

// The server's address as found by mDNS, or `fallback` if none was (ip or port 0). No path.
inline std::string server_base(uint32_t ip, uint16_t port, const std::string &fallback) {
  return (ip == 0 || port == 0) ? fallback : url(ip, port, "");
}

// "5 min", "3 h 20 min", for the out-of-date notice.
inline std::string age(uint32_t seconds) {
  uint32_t minutes = seconds / 60;
  if (minutes < 1)
    return "under 1 min";
  if (minutes < 60)
    return std::to_string(minutes) + " min";
  return std::to_string(minutes / 60) + " h " + std::to_string(minutes % 60) + " min";
}

// How far a zone can be from UTC: from -12:00 to +14:00. Anything else is not an offset the server meant.
constexpr int32_t MIN_UTC_OFFSET_S = -12 * 3600;
constexpr int32_t MAX_UTC_OFFSET_S = 14 * 3600;

inline bool valid_utc_offset(int64_t seconds) { return seconds >= MIN_UTC_OFFSET_S && seconds <= MAX_UTC_OFFSET_S; }

// The time of day, "14:32", at `unix_seconds` in a zone `utc_offset_s` ahead of UTC. The device has no timezone
// database, so the server says what the offset is (in its /plan) and this only adds it.
inline std::string clock_text(int64_t unix_seconds, int32_t utc_offset_s) {
  const int64_t local = unix_seconds + utc_offset_s;
  const int64_t of_day = ((local % 86400) + 86400) % 86400;
  char buffer[8];
  snprintf(buffer, sizeof buffer, "%02d:%02d", (int) (of_day / 3600), (int) (of_day % 3600 / 60));
  return buffer;
}

// The notice for a wake that failed, as a few words and, when it can be said truthfully, the time. `reason` is why
// (wifi, memory, or anything else). The time is the server's zone's: the clock's UTC plus the offset its last plan
// gave. Left off when the clock isn't set yet or no plan has ever answered, rather than shown wrong.
inline std::string failure_notice(eink_report::Failure reason, bool clock_set, int64_t unix_seconds, bool offset_known,
                                  int32_t utc_offset_s) {
  std::string what = "Last update failed";
  if (reason == eink_report::Failure::WIFI)
    what = "No Wi-Fi";
  else if (reason == eink_report::Failure::MEMORY)
    what = "Out of memory";
  return (clock_set && offset_known) ? what + " @ " + clock_text(unix_seconds, utc_offset_s) : what;
}

// The notice for an image the server says is out of date.
inline std::string stale_notice(uint32_t age_seconds) { return "Out of date: rendered " + age(age_seconds) + " ago"; }

}  // namespace eink_format
