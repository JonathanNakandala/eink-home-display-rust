// core/locate.h on the chip: the connection check (esp/reach.h) and the mDNS query (esp/discovery.h) put under it, with
// the time the watchdog has to allow for them (a check is up to 3 s, a search 3 queries of 2.5 s, and a server that has
// gone takes about 14 s of both).
#pragma once

#include <string>

#include "esphome/components/watchdog/watchdog.h"
#include "esphome/core/application.h"

#include "home_display/core/locate.h"
#include "home_display/core/service.h"
#include "home_display/esp/discovery.h"
#include "home_display/esp/reach.h"

namespace home_display_locate_chip {

// Whether the server is where it was, and if not where it is (core/locate.h says what each answer means).
inline home_display_locate::Where run(home_display_service::Transport transport, bool https_in_play,
                                      const std::string &instance_name, uint32_t ip, uint16_t port, uint16_t tls_port,
                                      home_display_service::Server &found) {
  esphome::watchdog::WatchdogManager wdm(40000);
  return home_display_locate::locate(
      https_in_play, ip, port, tls_port,
      [](uint32_t address, uint16_t on_port) {
        esphome::App.feed_wdt();
        return home_display_reach::connects(address, on_port);
      },
      [&](home_display_service::Server &server) {
        esphome::App.feed_wdt();
        return home_display_discovery::find(server, instance_name, transport);
      },
      found);
}

// A search of the network alone, as often as core/locate.h asks.
inline bool search(home_display_service::Transport transport, const std::string &instance_name,
                   home_display_service::Server &found) {
  esphome::watchdog::WatchdogManager wdm(40000);
  return home_display_locate::search(
      [&](home_display_service::Server &server) {
        esphome::App.feed_wdt();
        return home_display_discovery::find(server, instance_name, transport);
      },
      found);
}

}  // namespace home_display_locate_chip
