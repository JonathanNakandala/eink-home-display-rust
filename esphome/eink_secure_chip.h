// The secure transport on the chip: the pieces of it that are the chip's (NVS for flash, RTC memory for the one value
// that survives sleep, the system clock, ESPHome's watchdog), put together with the parts tested on a computer, so the
// wake script has a few calls to make.
//
// Only the chip's build has this. What it calls is eink_join.h, eink_secure_wake.h and the TLS under them, all run
// against a real server on a computer (host/).
#pragma once

#include <ctime>
#include <string>

#include "esp_attr.h"
#include "esphome/components/watchdog/watchdog.h"
#include "esphome/core/application.h"

#include "eink_base64.h"
#include "eink_clock.h"
#include "eink_est_client.h"
#include "eink_flash_identity.h"
#include "eink_join.h"
#include "eink_nvs.h"
#include "eink_pairing.h"
#include "eink_secure.h"
#include "eink_secure_begin.h"
#include "eink_secure_wake.h"
#include "eink_verifier.h"

namespace eink_secure_chip {

using Bytes = eink_ports::Bytes;

// The system clock as set by SNTP, believed only if eink_clock.h says it can be.
class SntpClock : public eink_ports::Clock {
 public:
  explicit SntpClock(uint32_t newest_render) : newest_render_(newest_render) {}
  int64_t now() override { return static_cast<int64_t>(::time(nullptr)); }
  bool usable() override { return eink_clock::usable(now(), newest_render_); }

 private:
  uint32_t newest_render_;
};

// What the server last said to this display (waiting, turned away): kept in RTC memory, which stays powered through
// deep sleep and is lost with the power, so a flat battery only means the display asks again. Checked before it is
// believed, since an update over the air leaves whatever the last firmware put there.
constexpr uint32_t RTC_MAGIC = 0xE1B70002;
static RTC_DATA_ATTR uint32_t rtc_magic = 0;
static RTC_DATA_ATTR eink_pairing::Answer rtc_answer = eink_pairing::Answer::NONE;

inline eink_pairing::Answer &last_answer() {
  if (rtc_magic != RTC_MAGIC ||
      static_cast<uint8_t>(rtc_answer) > static_cast<uint8_t>(eink_pairing::Answer::REFUSED)) {
    rtc_answer = eink_pairing::Answer::NONE;
    rtc_magic = RTC_MAGIC;
  }
  return rtc_answer;
}

inline eink_nvs::NvsStore &store() {
  static eink_nvs::NvsStore instance;
  return instance;
}

// The owner's root built into the firmware (`server_root`: the base64 of its DER), or empty, in which case it is
// fetched.
inline Bytes compiled_root(const char *server_root) {
  Bytes der;
  if (server_root == nullptr || server_root[0] == '\0' || !eink_base64::decode(server_root, der))
    return {};
  return der;
}

// The display's identity: its key, certificate and root in flash. One for the wake; the key lives in it.
inline eink_flash::FlashIdentity &identity(const char *server_root) {
  static eink_flash::FlashIdentity instance(store(), last_answer(), compiled_root(server_root));
  return instance;
}

// Joins if it is to, and decides how this wake reaches the server (eink_secure_begin.h), on the chip's clock and flash.
// `ip` and `tls_port` are what discovery found (0 for neither).
inline eink_secure_begin::Begin begin(eink_service::Transport transport, uint32_t ip, uint16_t tls_port,
                                      const std::string &name, uint32_t newest_render, const char *server_root) {
  // A handshake or two, and the key made on the first wake: more than a normal request, so more time before the
  // watchdog.
  esphome::watchdog::WatchdogManager wdm(120000);
  SntpClock clock(newest_render);
  return eink_secure_begin::begin(transport, ip, tls_port, name, clock, identity(server_root));
}

// A small request through the secure connection, with the time ESPHome's watchdog needs for it.
inline eink_secure::Fetched fetch(const std::string &method, const std::string &path) {
  esphome::watchdog::WatchdogManager wdm(70000);
  App.feed_wdt();
  eink_secure::Fetched result = eink_secure::fetch(method, path);
  App.feed_wdt();
  return result;
}

}  // namespace eink_secure_chip
