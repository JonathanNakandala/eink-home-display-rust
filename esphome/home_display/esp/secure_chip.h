// The secure transport on the chip: the pieces of it that are the chip's (NVS for flash, RTC memory for the one value
// that survives sleep, the system clock, ESPHome's watchdog), put together with the parts tested on a computer, so the
// wake script has a few calls to make.
//
// Only the chip's build has this. What it calls is core/join.h, core/secure_wake.h and the TLS under them, all run
// against a real server on a computer (tests/tls/).
#pragma once

#include <ctime>
#include <string>

#include "esp_attr.h"
#include "esphome/components/watchdog/watchdog.h"
#include "esphome/core/application.h"

#include "home_display/core/base64.h"
#include "home_display/core/clock.h"
#include "home_display/tls/est_client.h"
#include "home_display/tls/flash_identity.h"
#include "home_display/core/join.h"
#include "home_display/esp/nvs.h"
#include "home_display/esp/shown_etag.h"
#include "home_display/core/pairing.h"
#include "home_display/tls/secure.h"
#include "home_display/tls/secure_begin.h"
#include "home_display/core/session_cache.h"
#include "home_display/core/secure_wake.h"
#include "home_display/tls/verifier.h"

namespace home_display_secure_chip {

using Bytes = home_display_ports::Bytes;

// The system clock as set by SNTP, believed only if core/clock.h says it can be.
class SntpClock : public home_display_ports::Clock {
 public:
  explicit SntpClock(uint32_t newest_render) : newest_render_(newest_render) {}
  int64_t now() override { return static_cast<int64_t>(::time(nullptr)); }
  bool usable() override { return home_display_clock::usable(now(), newest_render_); }

 private:
  uint32_t newest_render_;
};

// The variables kept in RTC memory below are `inline`, not `static`: a static one would be a copy of its own in every
// file that includes this header, and a second file would silently see an empty answer and session after every sleep.
// Inline ones are one object whatever includes them.

// What the server last said to this display (waiting, turned away): kept in RTC memory, which stays powered through
// deep sleep and is lost with the power, so a flat battery only means the display asks again. Checked before it is
// believed, since an update over the air leaves whatever the last firmware put there.
constexpr uint32_t RTC_MAGIC = 0xE1B70002;
inline RTC_DATA_ATTR uint32_t rtc_magic = 0;
inline RTC_DATA_ATTR home_display_pairing::Answer rtc_answer = home_display_pairing::Answer::NONE;

inline home_display_pairing::Answer &last_answer() {
  if (rtc_magic != RTC_MAGIC ||
      static_cast<uint8_t>(rtc_answer) > static_cast<uint8_t>(home_display_pairing::Answer::REFUSED)) {
    rtc_answer = home_display_pairing::Answer::NONE;
    rtc_magic = RTC_MAGIC;
  }
  return rtc_answer;
}

// The TLS session kept between connections and wakes (core/session_cache.h), in RTC memory: it survives deep sleep and
// is lost with the power, which is right for something that only saves a handshake. What is in it is checked before it
// is believed, since an update over the air leaves whatever the last firmware put there.
inline RTC_DATA_ATTR home_display_session_cache::Slot rtc_session;

// The wall clock, if SNTP has set it: a session's age is worked out from it, because the clock mbedTLS uses starts
// again after deep sleep. -1 when it cannot be believed, and then nothing is kept or offered.
inline int64_t wall_clock() {
  const int64_t now = static_cast<int64_t>(::time(nullptr));
  return home_display_clock::plausible(now) ? now : -1;
}

inline home_display_session_cache::SlotStore<int64_t (*)()> &sessions() {
  static home_display_session_cache::SlotStore<int64_t (*)()> instance(rtc_session, home_display_session_cache::Key{},
                                                                       &wall_clock);
  return instance;
}

inline home_display_nvs::NvsStore &store() {
  static home_display_nvs::NvsStore instance;
  return instance;
}

// The owner's root built into the firmware (`server_root`: the base64 of its DER), or empty, in which case it is
// fetched.
inline Bytes compiled_root(const char *server_root) {
  Bytes der;
  if (server_root == nullptr || server_root[0] == '\0' || !home_display_base64::decode(server_root, der))
    return {};
  return der;
}

// The display's identity: its key, certificate and root in flash. One for the wake; the key lives in it.
inline home_display_flash::FlashIdentity &identity(const char *server_root) {
  static home_display_flash::FlashIdentity instance(store(), last_answer(), compiled_root(server_root));
  return instance;
}

// Erases the display's pairing, because the owner held the button for it: the key, the certificate and the root in
// flash, and what is kept in RTC memory beside them (the server's last answer, the TLS session, the picture's ETag).
// The built-in root (`server_root`) is part of the firmware and stays. Whatever state the flash record is in, it goes;
// nothing else removes pairing. True if it is all gone.
//
// Before anything has used the identity (`identity()` below keeps the key in memory for the wake), which is where the
// wake calls it: at the start, before it has joined or asked for anything.
inline bool erase_pairing() {
  home_display_credentials::Credentials credentials(store());
  const bool erased = credentials.erase();
  last_answer() = home_display_pairing::Answer::NONE;
  home_display_session_cache::clear(rtc_session);
  home_display_shown::forget();
  return erased;
}

// Joins if it is to, and decides how this wake reaches the server (tls/secure_begin.h), on the chip's clock and flash.
// `ip` and `tls_port` are what discovery found (0 for neither).
inline home_display_secure_begin::Begin begin(home_display_service::Transport transport, uint32_t ip, uint16_t tls_port,
                                              const std::string &name, uint32_t newest_render,
                                              const char *server_root) {
  // A handshake or two, and the key made on the first wake: more than a normal request, so more time before the
  // watchdog.
  esphome::watchdog::WatchdogManager wdm(120000);
  SntpClock clock(newest_render);
  return home_display_secure_begin::begin(transport, ip, tls_port, name, clock, identity(server_root), &sessions());
}

// A small request through the secure connection, with the time ESPHome's watchdog needs for it.
inline home_display_secure::Fetched fetch(const std::string &method, const std::string &path) {
  esphome::watchdog::WatchdogManager wdm(70000);
  App.feed_wdt();
  home_display_secure::Fetched result = home_display_secure::fetch(method, path);
  App.feed_wdt();
  return result;
}

}  // namespace home_display_secure_chip
