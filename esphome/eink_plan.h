// What a wake does with the server's /plan: whether the picture has to be fetched and drawn, and whether the
// zone offset it carries is one to keep.
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
// Reading the JSON is the YAML's job; this decides from what it read.
#pragma once

#include <cstdint>

#include "eink_format.h"

namespace eink_plan {

// What is on the panel besides the picture, and the battery's state.
struct Screen {
  bool notice_on_panel;     // a notice is covering (part of) the picture
  bool low_label_on_panel;  // the "Battery low" label is drawn
  bool battery_low;
  // The pairing code is on the panel (a notice) and the owner still has to type it in: leave it there, not redrawn
  // every wake, until the display is paired or the code is no longer wanted.
  bool prompt_kept = false;
};

// Whether the picture must be downloaded and drawn: the server says it changed (`changed`, false when the display
// said it already shows this version), or something else on the panel has to change with it.
//  - A notice is covering the picture and the server is fine again: redraw, even if the image is the one we
//    think we show. Not while `stale` (a scheduled render is long overdue), or it would redraw every wake; and not a
//    pairing code the owner still has to use.
//  - The battery label is part of the picture: draw it when it should appear or go.
inline bool needs_draw(bool changed, bool stale, const Screen &screen) {
  return changed || (screen.notice_on_panel && !stale && !screen.prompt_kept) ||
         screen.low_label_on_panel != screen.battery_low;
}

// Whether the offset a reply carried is one to keep: it was a number, and a real zone's offset. Otherwise the last
// one stays.
inline bool keeps_offset(bool was_a_number, int64_t utc_offset_s) {
  return was_a_number && eink_format::valid_utc_offset(utc_offset_s);
}

}  // namespace eink_plan
