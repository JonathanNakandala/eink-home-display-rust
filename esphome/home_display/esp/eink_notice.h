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
// `reset_dirty_region_`, `partial_update_count_`, `phase_` and, for `draw_picture`, the framebuffer and how it is laid
// out are protected members of the driver, reached with the usual explicit-instantiation trick. If a later ESPHome
// renames any of them, the build fails instead of wiping the screen at run time.
#pragma once

#include <cstdint>
#include <cstring>
#include <string>

#include "esp_rom_crc.h"
#include "esphome/components/display/display.h"
#include "esphome/components/image/image.h"
#include "esphome/components/it8951/it8951.h"
#include "esphome/core/application.h"
#include "esphome/core/log.h"

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

// What `draw_picture` needs of the framebuffer: where it is, how long a row is, how it is laid out, and the box that is
// redrawn.
struct FrameBuffer {
  using type = uint8_t *Display::*;
  friend type reach(FrameBuffer);
};
template struct Reach<FrameBuffer, &Display::buffer_>;

struct RowWidth {
  using type = uint16_t Display::*;
  friend type reach(RowWidth);
};
template struct Reach<RowWidth, &Display::row_width_>;

struct Grayscale {
  using type = bool Display::*;
  friend type reach(Grayscale);
};
template struct Reach<Grayscale, &Display::grayscale_>;

struct InvertColors {
  using type = bool Display::*;
  friend type reach(InvertColors);
};
template struct Reach<InvertColors, &Display::invert_colors_>;

struct EffectiveTransform {
  using type = uint8_t Display::*;
  friend type reach(EffectiveTransform);
};
template struct Reach<EffectiveTransform, &Display::effective_transform_>;

struct DirtyXLow {
  using type = uint16_t Display::*;
  friend type reach(DirtyXLow);
};
template struct Reach<DirtyXLow, &Display::x_low_>;
struct DirtyYLow {
  using type = uint16_t Display::*;
  friend type reach(DirtyYLow);
};
template struct Reach<DirtyYLow, &Display::y_low_>;
struct DirtyXHigh {
  using type = uint16_t Display::*;
  friend type reach(DirtyXHigh);
};
template struct Reach<DirtyXHigh, &Display::x_high_>;
struct DirtyYHigh {
  using type = uint16_t Display::*;
  friend type reach(DirtyYHigh);
};
template struct Reach<DirtyYHigh, &Display::y_high_>;

// Whether the driver has finished the update it was asked for. `component.update` only starts one: the driver then
// works through it a step at a time, one for each pass of the main loop. Started and left alone while the wake goes on
// to fetch a picture (a request that blocks the loop, then a download that decodes in long steps), a corner update took
// 81 seconds, and slowed the download until the watchdog gave up. So the wake waits for this first.
inline bool idle(Display &display) { return display.*reach(UpdatePhase()) == esphome::it8951::Phase::IDLE; }

// 1 to log both ways of drawing the picture and their checksums, and say whether they made the same framebuffer: the
// picture is drawn the slow way, the framebuffer checksummed and wiped, drawn the fast way, and checksummed again. For
// a bench build; the result is ~8 s slower.
#ifndef HOME_DISPLAY_VERIFY_PICTURE
#define HOME_DISPLAY_VERIFY_PICTURE 0
#endif

// The picture has to reach the driver's framebuffer, and a pixel at a time that costs: for each of the 2.6 million
// pixels `Image::draw` makes a virtual call to `draw_pixel_at`, which feeds the watchdog, checks the clipping, mirrors
// the position, widens the box that is redrawn, turns the colour into one of 16 greys and writes half a byte, and it
// goes down the picture a column at a time, which in PSRAM is a cache miss for each pixel. About 8 s of a wake.
//
// This writes the same framebuffer in one pass over the rows: each pair of pixels is one byte (the 4-bit grey of each,
// the one at the even position in the high half), and the picture is mirrored left to right, as the display is set to
// (`mirror_x`). It does that only for the picture it has been checked against: a grey picture with no transparency, the
// size of the panel, at the corner, on a grey panel with the colours as they are, mirrored and not turned, no clipping.
// Anything else is left to the slow way, which can do all of them.
inline void draw_picture(Display &display, esphome::image::Image &picture) {
  uint8_t *const buffer = display.*reach(FrameBuffer());
  const int width = display.get_width();
  const int height = display.get_height();
  const bool fast = buffer != nullptr && picture.get_data_start() != nullptr &&
                    picture.get_type() == esphome::image::IMAGE_TYPE_GRAYSCALE && !picture.has_transparency() &&
                    picture.get_width() == width && picture.get_height() == height && width % 2 == 0 &&
                    (display.*reach(Grayscale())) && !(display.*reach(InvertColors())) &&
                    (display.*reach(EffectiveTransform())) == esphome::it8951::TRANSFORM_MIRROR_X &&
                    !display.get_clipping().is_set() && static_cast<int>(display.*reach(RowWidth())) == width / 2;
  if (!fast) {
    display.image(0, 0, &picture);
    return;
  }
  const size_t buffer_length = static_cast<size_t>(width / 2) * height;
#if HOME_DISPLAY_VERIFY_PICTURE
  const uint32_t slow_started = esphome::millis();
  display.image(0, 0, &picture);
  const uint32_t slow_ms = esphome::millis() - slow_started;
  const uint32_t slow_crc = esp_rom_crc32_le(0, buffer, buffer_length);
  memset(buffer, 0x5A, buffer_length);  // so a byte the fast way leaves alone shows up as a difference
  const uint32_t fast_started = esphome::millis();
#endif
  // The 4-bit grey of an 8-bit one, as the driver makes it: (g + 8) / 16, and not more than 15.
  auto nibble = [](uint8_t grey) -> uint8_t {
    const uint8_t n = static_cast<uint8_t>((grey + 8) >> 4);
    return static_cast<uint8_t>(n - (n >> 4));
  };
  const uint8_t *source = picture.get_data_start();
  const int row_bytes = width / 2;
  for (int y = 0; y < height; y++) {
    if ((y & 63) == 0)
      esphome::App.feed_wdt();
    const uint8_t *row = source + static_cast<size_t>(y) * width;
    uint8_t *out = buffer + static_cast<size_t>(y) * row_bytes;
    // Mirrored: the byte at `j` holds the panel's columns 2j (high half) and 2j + 1, which are the picture's columns
    // width - 1 - 2j and width - 2 - 2j.
    const uint8_t *p = row + width - 1;
    for (int j = 0; j < row_bytes; j++, p -= 2)
      out[j] = static_cast<uint8_t>((nibble(p[0]) << 4) | nibble(p[-1]));
  }
  // The whole panel is what is drawn, as it is by the pixel-by-pixel way.
  display.*reach(DirtyXLow()) = 0;
  display.*reach(DirtyYLow()) = 0;
  display.*reach(DirtyXHigh()) = static_cast<uint16_t>(width);
  display.*reach(DirtyYHigh()) = static_cast<uint16_t>(height);
#if HOME_DISPLAY_VERIFY_PICTURE
  const uint32_t fast_ms = esphome::millis() - fast_started;
  const uint32_t fast_crc = esp_rom_crc32_le(0, buffer, buffer_length);
  ESP_LOGI("picture", "Drawn by pixel in %u ms (framebuffer CRC-32 %08x), in one pass in %u ms (%08x): %s",
           (unsigned) slow_ms, (unsigned) slow_crc, (unsigned) fast_ms, (unsigned) fast_crc,
           slow_crc == fast_crc ? "the same" : "DIFFERENT");
#endif
}

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
