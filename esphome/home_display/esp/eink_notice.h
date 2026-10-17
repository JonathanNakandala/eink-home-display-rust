// Writes a short notice in the bottom-right corner of the e-ink panel without redrawing the picture.
//
// After deep sleep the device holds no copy of the picture; it only exists on the panel. The
// it8951 driver refreshes just the area drawn to since the last update, but two things make a
// plain update of a freshly booted display blank the panel (the whole of it is redrawn from a
// framebuffer that is empty):
//  - it marks the whole screen as changed at start-up. This clears that mark first.
//  - it seeds its update counter so that the very first update after boot is a full-panel one,
//    which ignores the changed area altogether (`set_full_update_every`, to give a fresh panel a
//    clean refresh). This sets the counter back, so the notice's update counts as a partial one.
// Then only the notice is refreshed and the rest of the screen keeps what it was showing. (Seen
// on the reTerminal: without the second, every notice drew 1872x1404 and left the notice alone on
// a white panel.)
//
// `reset_dirty_region_`, `partial_update_count_` and `phase_` are protected members of the driver, reached
// with the usual explicit-instantiation trick. If a later ESPHome renames any of them, the build
// fails instead of wiping the screen at run time.
#pragma once

#include <cstdint>
#include <string>

#include "esphome/components/display/display.h"
#include "esphome/components/it8951/it8951.h"

namespace eink_notice {

using Display = esphome::it8951::IT8951Display;

template <typename Tag, typename Tag::type Member> struct Reach {
  friend typename Tag::type reach(Tag) { return Member; }
};
struct ResetDirtyRegion {
  using type = void (Display::*)();
  friend type reach(ResetDirtyRegion);
};
template struct Reach<ResetDirtyRegion, &Display::reset_dirty_region_>;

struct PartialUpdateCount {
  using type = uint32_t Display::*;
  friend type reach(PartialUpdateCount);
};
template struct Reach<PartialUpdateCount, &Display::partial_update_count_>;

struct UpdatePhase {
  using type = esphome::it8951::Phase Display::*;
  friend type reach(UpdatePhase);
};
template struct Reach<UpdatePhase, &Display::phase_>;

// Whether the driver has finished the update it was asked for. `component.update` only starts one: the driver then works
// through it a step at a time, one for each pass of the main loop. Started and left alone while the wake goes on to
// fetch a picture (a request that blocks the loop, then a download that decodes in long steps), a corner update took
// 81 seconds, and slowed the download until the watchdog gave up. So the wake waits for this first.
inline bool idle(Display &display) { return display.*reach(UpdatePhase()) == esphome::it8951::Phase::IDLE; }

// Draws `text` on a white label in the bottom-right corner, `margin` pixels from the edges.
// Drawn over whatever is on the display, so use it as part of drawing a whole picture.
inline void label(esphome::display::Display &display, esphome::display::BaseFont *font, const std::string &text,
                  int margin = 24, int padding = 12) {
  const int x = display.get_width() - margin;
  const int y = display.get_height() - margin;
  int x1, y1, w, h;
  display.get_text_bounds(x, y, text.c_str(), font, esphome::display::TextAlign::BOTTOM_RIGHT, &x1, &y1, &w, &h);
  // The label covers whatever is underneath, so the text stays legible over the picture.
  display.filled_rectangle(x1 - padding, y1 - padding, w + 2 * padding, h + 2 * padding, esphome::Color::WHITE);
  display.print(x, y, font, esphome::Color::BLACK, esphome::display::TextAlign::BOTTOM_RIGHT, text.c_str());
}

// Replaces just the label's corner of what is already on the panel, leaving the rest alone.
inline void draw(Display &display, esphome::display::BaseFont *font, const std::string &text, int margin = 24,
                 int padding = 12) {
  (display.*reach(ResetDirtyRegion()))();
  // Run inside the update, before the driver counts it, so this one is not the first.
  display.*reach(PartialUpdateCount()) = 0;
  label(display, font, text, margin, padding);
}

}  // namespace eink_notice
