// The arithmetic of a wake: how long to sleep, how the backoff grows, how much memory the image needs.
//
// The YAML only reads and writes its globals and calls these.
// What is device-specific (the heap, the clock, deep sleep itself) stays in the YAML and in eink_health.h.
#pragma once

#include <cstddef>
#include <cstdint>

namespace eink_wake {

// A little more than a minute at least: the device never sleeps less, whatever the server says.
constexpr uint32_t MIN_SLEEP_MS = 60 * 1000u;
// And never more than a day, which is also the longest the server will ask for.
constexpr uint32_t MAX_SLEEP_MS = 24 * 3600 * 1000u;

// The server's `next_seconds`, as milliseconds the device will sleep: between a minute and a day.
inline uint32_t plan_sleep_ms(uint32_t next_seconds) {
  constexpr uint32_t min_s = MIN_SLEEP_MS / 1000, max_s = MAX_SLEEP_MS / 1000;
  return (next_seconds < min_s ? min_s : next_seconds > max_s ? max_s : next_seconds) * 1000u;
}

// One more failed wake in a row. Saturates, so a long outage can't wrap the count back to "first failure".
inline uint8_t count_failure(uint8_t failed_wakes) { return failed_wakes < 255 ? failed_wakes + 1 : failed_wakes; }

// How long to sleep after the `failed_wakes`-th failure in a row (1 for the first): `base_ms`, doubling with
// each one, up to `max_ms`. The doubling stops after six, so it can't overflow whatever `base_ms` is.
inline uint32_t backoff_ms(uint8_t failed_wakes, uint32_t base_ms, uint32_t max_ms) {
  const uint32_t shift = failed_wakes > 7 ? 6 : failed_wakes == 0 ? 0 : failed_wakes - 1;
  const uint64_t ms = (uint64_t) base_ms << shift;
  return ms > max_ms ? max_ms : (uint32_t) ms;
}

// The sleep that is left of what the server asked for. The server counted from when it answered (at
// `received_ms` on this wake's clock), and the wake has used some of that since: the download, the refresh,
// the settle. Without taking it off, every wake would land late. `received_ms` of 0 means the sleep isn't the
// server's plan (a backoff, or no plan), so it is used as it is. Never less than a minute.
// The clock is a 32-bit millisecond counter that wraps; the difference is right across a wrap.
inline uint32_t sleep_after(uint32_t asked_ms, uint32_t received_ms, uint32_t now_ms) {
  if (received_ms == 0)
    return asked_ms;
  const uint32_t used = now_ms - received_ms;
  return asked_ms > used && asked_ms - used > MIN_SLEEP_MS ? asked_ms - used : MIN_SLEEP_MS;
}

// Bytes the decoded image needs: one per pixel for a GRAYSCALE online_image, plus `margin` for the download
// buffer and decoder state.
inline size_t image_bytes_needed(uint32_t width, uint32_t height, size_t margin) {
  return (size_t) width * height + margin;
}

}  // namespace eink_wake
