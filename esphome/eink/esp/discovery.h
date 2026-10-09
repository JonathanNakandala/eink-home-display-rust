// Finds the eink-home-display-rust image server on the LAN with an mDNS / DNS-SD query.
// The server announces itself as `_http._tcp` (or `_https._tcp` when it serves only HTTPS) with the TXT keys txtvers,
// path, format (served now), formats (all it can serve) and, when it offers HTTPS, tlsport and secure (see
// src/adapters/image_server/advertise.rs in the Rust app). What to make of an answer is core/service.h, which is
// tested on a computer; this is only the query.
#pragma once

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>

#include "esphome/core/log.h"
#include "mdns.h"

#include "eink/core/service.h"

namespace eink_discovery {

static const char *const TAG = "eink_discovery";

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

inline bool first_ipv4(const mdns_result_t *result, uint32_t &ip) {
  for (const mdns_ip_addr_t *a = result->addr; a != nullptr; a = a->next) {
    if (a->addr.type == ESP_IPADDR_TYPE_V4) {
      ip = a->addr.u_addr.ip4.addr;
      return true;
    }
  }
  return false;
}

// One answer of the scan as the text core/service.h judges.
inline eink_service::Announcement announcement(const mdns_result_t *result) {
  eink_service::Announcement a;
  if (result->instance_name != nullptr)
    a.instance = result->instance_name;
  a.port = result->port;
  first_ipv4(result, a.ip);
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
inline bool find(eink_service::Server &server, const std::string &instance_name, eink_service::Transport transport,
                 uint32_t timeout_ms = 2500) {
  mdns_result_t *results = nullptr;
  esp_err_t err = mdns_query_ptr(eink_service::service_type(transport), "_tcp", timeout_ms, 20, &results);
  if (err != ESP_OK) {
    ESP_LOGW(TAG, "mDNS query failed: %s", esp_err_to_name(err));
    return false;
  }

  bool found = false;
  for (const mdns_result_t *r = results; r != nullptr && !found; r = r->next) {
    // Plenty of things on a LAN are _http._tcp; ours says so in its TXT record.
    found = eink_service::accept(announcement(r), instance_name, transport, server);
    if (found)
      ESP_LOGI(TAG, "Found '%s' at %s (http %u, https %u, format %s)", r->instance_name ? r->instance_name : "?",
               r->hostname ? r->hostname : "?", server.port, server.tls_port, server.format.c_str());
  }
  mdns_query_results_free(results);
  if (!found)
    ESP_LOGW(TAG, "No image server found");
  return found;
}

}  // namespace eink_discovery
