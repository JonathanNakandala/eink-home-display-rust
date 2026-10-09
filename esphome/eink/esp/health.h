// Cheap checks made before a step that can fail in a way the log explains badly.
#pragma once

#include <cstddef>
#include <cstdint>

#include "esp_heap_caps.h"
#include "esphome/core/log.h"

#include "eink/core/wake.h"

namespace eink_health {

static const char *const TAG = "eink_health";

// Whether the decoded image (one byte per pixel for a GRAYSCALE online_image) can be allocated.
// online_image allocates it in one block from PSRAM, so the largest free block is what matters,
// not the total. `margin` leaves room for the download buffer and decoder state.
inline bool image_buffer_fits(uint32_t width, uint32_t height, size_t margin = 96 * 1024) {
  const size_t needed = eink_wake::image_bytes_needed(width, height, margin);
  const size_t largest = heap_caps_get_largest_free_block(MALLOC_CAP_SPIRAM);
  if (largest >= needed)
    return true;
  ESP_LOGE(TAG, "Not enough PSRAM for the image: need %u bytes in one block, largest free block is %u (total free %u)",
           (unsigned) needed, (unsigned) largest, (unsigned) heap_caps_get_free_size(MALLOC_CAP_SPIRAM));
  return false;
}

}  // namespace eink_health
