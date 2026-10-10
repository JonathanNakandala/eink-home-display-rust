// What a display that prefers HTTPS remembers when the server it found does not serve it: not to look again at every
// wake.
//
// Without it, a display set to `prefer-https` in front of a server with no HTTPS port would, at every wake, wait for
// the clock (up to 20 s of radio), look the server up again because its HTTPS port stays 0, decide there is nothing to
// join, and tell the server it had failed. With it, the first search that finds the server without HTTPS is remembered
// for a day, and the wakes in between go straight to plain HTTP; after a day it looks again, in case the server has
// been set up for HTTPS since.
//
// Only a search that found the server and saw no HTTPS port is remembered. A server that was not found, or whose HTTPS
// did not answer, is a failure to try again at the next wake, not an absence to remember.
//
// A day is counted in sleep, not by the clock: the display adds up how long it sleeps, which needs no time to be set,
// and the memory is in RTC memory, which a power loss clears (it then looks once, as a new display does).
#pragma once

#include <cstdint>

namespace home_display_no_https {

constexpr uint32_t MAGIC = 0xE1B70006;
constexpr uint32_t REMEMBER_MS = 24u * 60 * 60 * 1000;

struct Memory {
  uint32_t magic;
  uint32_t remaining_ms;  // how much more sleep the memory lasts
};

constexpr Memory EMPTY = {0, 0};

inline void forget(Memory &memory) { memory = EMPTY; }

inline void remember(Memory &memory) { memory = {MAGIC, REMEMBER_MS}; }

// Whether it is remembered that the server has no HTTPS. Memory another firmware left, or nothing, is not believed.
inline bool remembers(const Memory &memory) { return memory.magic == MAGIC && memory.remaining_ms > 0; }

// A sleep of `ms` has passed. The memory is spent when a day has.
inline void slept(Memory &memory, uint32_t ms) {
  if (!remembers(memory))
    return;
  if (ms >= memory.remaining_ms)
    forget(memory);
  else
    memory.remaining_ms -= ms;
}

}  // namespace home_display_no_https
