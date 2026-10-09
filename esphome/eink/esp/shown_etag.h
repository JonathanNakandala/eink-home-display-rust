// The ETag of the picture on the panel, in RTC memory (core/etag.h): the display asks for the next picture with it, and
// a server that has the same bytes answers 304, so the picture is neither downloaded nor drawn again.
//
// RTC memory survives deep sleep and is lost with the power, which only means one download in full. Inline, so that it
// is one object whatever includes this header (see esp/secure_chip.h).
#pragma once

#include <string>

#include "esp_attr.h"

#include "eink/core/etag.h"

namespace eink_shown {

inline RTC_DATA_ATTR eink_etag::Slot slot;

// What to ask the next picture with (`If-None-Match`), or "" to ask without: see eink_etag::condition.
inline std::string condition(bool panel_shows_only_the_picture) {
  return eink_etag::condition(slot, panel_shows_only_the_picture);
}

// The picture just drawn had this ETag ("" if the server sent none, or it did not come over TLS: nothing is then asked
// about it later).
inline void keep(const std::string &etag) { eink_etag::keep(slot, etag); }

// The panel no longer shows a picture that is known (the display was reset): nothing is asked about it.
inline void forget() { eink_etag::clear(slot); }

}  // namespace eink_shown
