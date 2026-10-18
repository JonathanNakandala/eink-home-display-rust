#pragma once

// Where a picture's decode time goes, as CPU cycle counts added up over the picture and logged with "Decoding complete".
// (A local change; see the README beside this file.)
//
// The cycle counter is one instruction to read, so timing every pixel costs a few per cent and does not move the answer the way
// a trace over a serial line does. The shares are what to trust; the totals run a little high. Set RUNTIME_IMAGE_PROFILE to 0
// to leave all of it out.

#ifndef RUNTIME_IMAGE_PROFILE
#define RUNTIME_IMAGE_PROFILE 1
#endif

// A run of one colour (QOI) is filled into the picture in one go, in place of a call for each pixel. 0 goes back to the call for
// each pixel, which makes the same picture (the profile logs a checksum of it, to compare).
#ifndef RUNTIME_IMAGE_FILL_RUNS
#define RUNTIME_IMAGE_FILL_RUNS 1
#endif

// 1 logs a CRC-32 of the decoded picture, to show that a change to how it is painted has not changed it. It reads the whole
// picture (157 ms for 2.6 MB), so it is off unless a bench build wants it.
#ifndef RUNTIME_IMAGE_CHECKSUM
#define RUNTIME_IMAGE_CHECKSUM 0
#endif

#if RUNTIME_IMAGE_PROFILE

#include <cstddef>
#include <cstdint>

#include "esp_cpu.h"
#include "esphome/core/log.h"

namespace esphome::runtime_image {

struct DecodeProfile {
  uint32_t draw_cycles = 0;       // inside ImageDecoder::draw, all of it
  uint32_t pixel_cycles = 0;      // inside RuntimeImage::draw_pixel, which draw calls (the grey sum and the store)
  uint32_t draw_calls = 0;
  uint32_t decode_calls = 0;      // one for each chunk of the download handed to the decoder
  uint32_t decode_bytes_min = 0xFFFFFFFF;
  uint32_t decode_bytes_max = 0;
  uint32_t run_chunks = 0;        // QOI: runs of a colour
  uint32_t run_pixels = 0;
  uint32_t other_chunks = 0;      // QOI: every other way of saying one pixel

  void reset() { *this = DecodeProfile(); }

  void note_decode_call(size_t bytes) {
    decode_calls++;
    if (bytes < decode_bytes_min)
      decode_bytes_min = bytes;
    if (bytes > decode_bytes_max)
      decode_bytes_max = bytes;
  }

  // `decode_us` is the time the decoder was entered for, from the microsecond clock; the rest is from the cycle counter.
  void log(const char *tag, uint32_t decode_us) const {
    const uint32_t cpu_mhz = 240;  // the ESP32-S3 as ESPHome sets it
    const uint32_t decode_cycles = decode_us * cpu_mhz;
    auto ms = [&](uint32_t cycles) { return (unsigned) (cycles / (cpu_mhz * 1000)); };
    auto pct = [&](uint32_t cycles) { return decode_cycles ? (unsigned) ((uint64_t) cycles * 100 / decode_cycles) : 0u; };
    const uint32_t overhead = draw_cycles - pixel_cycles;     // the call, the clipping and the loop, not the store
    const uint32_t parsing = decode_cycles - draw_cycles;     // reading chunks, the colour table, the position sums
    ESP_LOGI(tag, "Decoder time: %u ms in all = %u ms parsing (%u%%) + %u ms drawing (%u%%): %u ms in draw() itself (%u%%) "
                  "+ %u ms in draw_pixel (%u%%, the grey sum and the store)",
             (unsigned) (decode_us / 1000), ms(parsing), pct(parsing), ms(draw_cycles), pct(draw_cycles), ms(overhead),
             pct(overhead), ms(pixel_cycles), pct(pixel_cycles));
    ESP_LOGI(tag, "Decoder work: %u draw calls, %u chunks from the network (%u to %u bytes), QOI %u runs covering %u pixels and "
                  "%u other chunks",
             (unsigned) draw_calls, (unsigned) decode_calls, (unsigned) (decode_calls ? decode_bytes_min : 0),
             (unsigned) decode_bytes_max, (unsigned) run_chunks, (unsigned) run_pixels, (unsigned) other_chunks);
  }
};

inline DecodeProfile decode_profile;

// Adds the cycles from its making to its end to `total`.
struct CycleScope {
  uint32_t &total;
  uint32_t start;
  explicit CycleScope(uint32_t &t) : total(t), start(esp_cpu_get_cycle_count()) {}
  ~CycleScope() { total += esp_cpu_get_cycle_count() - start; }
};

}  // namespace esphome::runtime_image

#define RUNTIME_IMAGE_PROFILE_SCOPE(name, total) ::esphome::runtime_image::CycleScope name(total)
#define RUNTIME_IMAGE_PROFILE_DO(...) __VA_ARGS__

#else

#define RUNTIME_IMAGE_PROFILE_SCOPE(name, total)
#define RUNTIME_IMAGE_PROFILE_DO(...)

#endif
