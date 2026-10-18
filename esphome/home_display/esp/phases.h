// Where a wake's time goes: a line in the log as each phase ends, and a table of them all just before sleeping.
//
// For finding out what is slow, not for the device to act on, so it keeps nothing across a sleep and costs a few
// hundred bytes. The times are milliseconds since boot, which is the start of the wake (the first ~300 ms, the
// bootloader's, is before that and not counted). A phase's length is the time since the one before it, so the marks
// are placed where a phase ends: the log reads "wifi 1840 ms (+1620)", the Wi-Fi taking 1620 ms.
#pragma once

#include <cstdint>

#include "esphome/core/hal.h"
#include "esphome/core/log.h"

namespace home_display_phases {

constexpr const char *TAG = "phase";
constexpr uint8_t MAX_MARKS = 24;

struct Mark {
  const char *name;  // a string literal
  uint32_t at_ms;
};

inline Mark marks[MAX_MARKS];
inline uint8_t count = 0;

// Says a phase has just ended. `name` must be a string literal. Marks past the 24th are dropped.
inline void mark(const char *name) {
  const uint32_t now = esphome::millis();
  const uint32_t before = count ? marks[count - 1].at_ms : 0;
  ESP_LOGI(TAG, "%-16s %6u ms  (+%u)", name, (unsigned) now, (unsigned) (now - before));
  if (count < MAX_MARKS)
    marks[count++] = {name, now};
}

// One line for each phase, in order, with the share of the wake each took. Called as the wake ends.
inline void summary() {
  mark("before-sleep");
  const uint32_t total = marks[count - 1].at_ms;
  ESP_LOGI(TAG, "---- where the %u ms of this wake went ----", (unsigned) total);
  uint32_t before = 0;
  for (uint8_t i = 0; i < count; i++) {
    const uint32_t took = marks[i].at_ms - before;
    ESP_LOGI(TAG, "%-16s %6u ms  %3u%%", marks[i].name, (unsigned) took,
             total ? (unsigned) (took * 100ULL / total) : 0);
    before = marks[i].at_ms;
  }
}

}  // namespace home_display_phases
