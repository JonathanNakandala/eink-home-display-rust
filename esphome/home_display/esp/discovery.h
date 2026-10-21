// Finds the home-display-server image server on the LAN with an mDNS / DNS-SD query.
// The server announces itself as `_http._tcp` (or `_https._tcp` when it serves only HTTPS) with the TXT keys txtvers,
// path, format (served now), formats (all it can serve) and, when it offers HTTPS, tlsport and secure (see
// src/adapters/image_server/advertise.rs in the Rust app). What to make of an answer is core/service.h, which is
// tested on a computer; this is only the query.
#pragma once

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "esp_netif.h"
#include "esphome/core/log.h"
#include "mdns.h"

#include "home_display/core/service.h"

namespace home_display_discovery {

static const char *const TAG = "home_display_discovery";

inline std::string txt_value(const mdns_result_t *result, const char *key) {
  for (size_t i = 0; i < result->txt_count; i++) {
    if (strcmp(result->txt[i].key, key) != 0 || result->txt[i].value == nullptr)
      continue;
    // The lengths are kept in a separate array, which is absent for plain items.
    if (result->txt_value_len != nullptr)
      return std::string(result->txt[i].value, result->txt_value_len[i]);
    return result->txt[i].value;
  }
  return "";
}

// The address and netmask of the display's own network, as lwIP stores them (0 and 0 if it has none).
inline void own_network(uint32_t &ip, uint32_t &netmask) {
  ip = 0;
  netmask = 0;
  esp_netif_t *netif = esp_netif_get_handle_from_ifkey("WIFI_STA_DEF");
  esp_netif_ip_info_t info;
  if (netif != nullptr && esp_netif_get_ip_info(netif, &info) == ESP_OK) {
    ip = info.ip.addr;
    netmask = info.netmask.addr;
  }
}

// The server's address: of those it announced, the one on the display's own network (core/service.h `choose_ip`). A
// machine with a second adapter, a VPN or a container bridge announces an address on each, and the first is not always
// one the display can reach.
inline uint32_t chosen_ipv4(const mdns_result_t *result) {
  std::vector<uint32_t> all;
  for (const mdns_ip_addr_t *a = result->addr; a != nullptr; a = a->next) {
    if (a->addr.type == ESP_IPADDR_TYPE_V4)
      all.push_back(a->addr.u_addr.ip4.addr);
  }
  uint32_t own_ip, netmask;
  own_network(own_ip, netmask);
  return home_display_service::choose_ip(all, own_ip, netmask);
}

// One answer of the scan as the text core/service.h judges.
inline home_display_service::Announcement announcement(const mdns_result_t *result) {
  home_display_service::Announcement a;
  if (result->instance_name != nullptr)
    a.instance = result->instance_name;
  a.port = result->port;
  a.ip = chosen_ipv4(result);
  a.txtvers = txt_value(result, "txtvers");
  a.path = txt_value(result, "path");
  a.format = txt_value(result, "format");
  a.tlsport = txt_value(result, "tlsport");
  a.secure = txt_value(result, "secure");
  return a;
}

// Blocks for up to `timeout_ms`. `instance_name`, when not empty, picks one server by the name shown in a scan (the
// Rust app's server.instance_name), for networks with several. `transport` is how this display is set to reach the
// server, which decides where to look (`_https._tcp` only for HTTPS, else `_http._tcp`) and what the port means.
inline bool find(home_display_service::Server &server, const std::string &instance_name,
                 home_display_service::Transport transport, uint32_t timeout_ms = 2500) {
  mdns_result_t *results = nullptr;
  esp_err_t err = mdns_query_ptr(home_display_service::service_type(transport), "_tcp", timeout_ms, 20, &results);
  if (err != ESP_OK) {
    ESP_LOGW(TAG, "mDNS query failed: %s", esp_err_to_name(err));
    return false;
  }

  bool found = false;
  for (const mdns_result_t *r = results; r != nullptr && !found; r = r->next) {
    // Plenty of things on a LAN are _http._tcp; ours says so in its TXT record.
    found = home_display_service::accept(announcement(r), instance_name, transport, server);
    if (found)
      ESP_LOGI(TAG, "Found '%s' at %s (http %u, https %u, format %s)", r->instance_name ? r->instance_name : "?",
               r->hostname ? r->hostname : "?", server.port, server.tls_port, server.format.c_str());
  }
  mdns_query_results_free(results);
  if (!found)
    ESP_LOGW(TAG, "No image server found");
  return found;
}

}  // namespace home_display_discovery
