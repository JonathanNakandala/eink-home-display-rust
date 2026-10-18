// KEY0, the right green button, read directly.
//
// It is also the pin that wakes the display from deep sleep (`esp32_ext1_wakeup` in packages/wake.yaml), and ESPHome
// does not let a binary sensor share a pin with that ("Pin 3 is used in multiple places"). So it is read here, from
// ESP-IDF, when the wake begins. After a wake by this pin the pad is still under the sleep's control; it is handed back
// before it is read.
#pragma once

#include "driver/gpio.h"
#include "driver/rtc_io.h"
#include "esp_idf_version.h"
#include "esp_sleep.h"

namespace home_display_button {

constexpr gpio_num_t KEY0 = GPIO_NUM_3;

// Whether this wake was KEY0 pulling the pin low in deep sleep, and not the timer or a reset. ESP-IDF 6 replaced the single
// cause by a bitmap of them (a wake can have several), and deprecated the old call.
inline bool woken_by_key0() {
#if ESP_IDF_VERSION >= ESP_IDF_VERSION_VAL(6, 0, 0)
  return (esp_sleep_get_wakeup_causes() & (1u << ESP_SLEEP_WAKEUP_EXT1)) != 0;
#else
  return esp_sleep_get_wakeup_cause() == ESP_SLEEP_WAKEUP_EXT1;
#endif
}

// Whether KEY0 is held down now (the button pulls the pin to ground).
inline bool key0_down() {
  static bool ready = false;
  if (!ready) {
    rtc_gpio_deinit(KEY0);
    gpio_config_t config = {};
    config.pin_bit_mask = 1ULL << KEY0;
    config.mode = GPIO_MODE_INPUT;
    config.pull_up_en = GPIO_PULLUP_ENABLE;
    config.pull_down_en = GPIO_PULLDOWN_DISABLE;
    config.intr_type = GPIO_INTR_DISABLE;
    ready = gpio_config(&config) == ESP_OK;
  }
  return ready && gpio_get_level(KEY0) == 0;
}

}  // namespace home_display_button
