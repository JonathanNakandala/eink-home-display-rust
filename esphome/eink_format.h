// Text the device builds: a server's URL from what mDNS found, an age in words, and the time of day.
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
#pragma once

#include <cstdint>
#include <cstdio>
#include <string>

namespace eink_format {

// `ip` is IPv4 in the byte order lwIP stores it (first octet in the lowest byte).
inline std::string url(uint32_t ip, uint16_t port, const std::string &path) {
  char buffer[64];
  snprintf(buffer, sizeof buffer, "http://%u.%u.%u.%u:%u", (unsigned) (ip & 0xff), (unsigned) ((ip >> 8) & 0xff),
           (unsigned) ((ip >> 16) & 0xff), (unsigned) ((ip >> 24) & 0xff), (unsigned) port);
  return buffer + path;
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

}  // namespace eink_format
