// Text the device builds: a server's URL from what mDNS found, and an age in words.
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

}  // namespace eink_format
