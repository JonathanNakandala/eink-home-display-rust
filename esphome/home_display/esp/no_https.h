// Whether the server has no HTTPS, as remembered between wakes (core/no_https.h), in RTC memory: it survives deep sleep
// and is lost with the power, which only means one more search. Inline, so that it is one object whatever includes this
// header (see esp/secure_chip.h).
#pragma once

#include "esp_attr.h"

#include "home_display/core/no_https.h"
#include "home_display/core/secure_wake.h"

namespace home_display_no_https {

inline RTC_DATA_ATTR Memory memory = EMPTY;

inline bool remembers() { return remembers(memory); }

// Takes what a search for the server found into the memory.
inline void apply(home_display_secure_wake::Memo memo) {
  using home_display_secure_wake::Memo;
  if (memo == Memo::REMEMBER)
    remember(memory);
  else if (memo == Memo::FORGET)
    forget(memory);
}

// The display is about to sleep for `ms`.
inline void slept(uint32_t ms) { slept(memory, ms); }

}  // namespace home_display_no_https
