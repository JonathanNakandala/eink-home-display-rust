// Writes a short notice in the bottom-right corner of the e-paper without redrawing the picture.
//
// After deep sleep the device holds no copy of the picture; it only exists on the panel. The
// it8951 driver refreshes just the area drawn to since the last update, but it marks the whole
// screen as changed at start-up, so a plain update would blank the panel. This clears that mark
// first, so only the notice is refreshed and the rest of the screen keeps what it was showing.
//
// `reset_dirty_region_` is a protected member of the driver, reached with the usual
// explicit-instantiation trick. If a later ESPHome renames it, the build fails instead of
// wiping the screen at run time.
#pragma once

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
  label(display, font, text, margin, padding);
}

}  // namespace eink_notice
