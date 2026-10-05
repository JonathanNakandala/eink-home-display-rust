// Finds the eink-home-display-rust image server on the LAN with an mDNS / DNS-SD query.
// The server announces itself as `_http._tcp` with the TXT keys txtvers, path and format
// (see src/adapters/image_server/advertise.rs in the Rust app).
#pragma once

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>

#include "esphome/core/log.h"
#include "mdns.h"

namespace eink_discovery {

static const char *const TAG = "eink_discovery";

struct Server {
  uint32_t ip = 0;  // IPv4, in the byte order lwIP stores it (first octet in the lowest byte)
  uint16_t port = 0;
  std::string format;  // "bmp" or "png", as announced
};

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

// Blocks for up to `timeout_ms`. `instance_name`, when not empty, picks one server by
// the name shown in a scan (the Rust app's server.instance_name), for networks with several.
inline bool find(Server &server, const std::string &instance_name, uint32_t timeout_ms = 2500) {
  mdns_result_t *results = nullptr;
  esp_err_t err = mdns_query_ptr("_http", "_tcp", timeout_ms, 20, &results);
  if (err != ESP_OK) {
    ESP_LOGW(TAG, "mDNS query failed: %s", esp_err_to_name(err));
    return false;
  }

  bool found = false;
  for (const mdns_result_t *r = results; r != nullptr && !found; r = r->next) {
    // Plenty of things on a LAN are _http._tcp; ours says so in its TXT record.
    if (txt_value(r, "txtvers") != "1" || txt_value(r, "path").empty())
      continue;
    if (!instance_name.empty() && (r->instance_name == nullptr || instance_name != r->instance_name))
      continue;
    uint32_t ip;
    if (r->port == 0 || !first_ipv4(r, ip))
      continue;
    server.ip = ip;
    server.port = r->port;
    server.format = txt_value(r, "format");
    found = true;
    ESP_LOGI(TAG, "Found '%s' at %s:%u (format %s)", r->instance_name ? r->instance_name : "?",
             r->hostname ? r->hostname : "?", r->port, server.format.c_str());
  }
  mdns_query_results_free(results);
  if (!found)
    ESP_LOGW(TAG, "No image server found");
  return found;
}

inline std::string url(uint32_t ip, uint16_t port, const std::string &path) {
  char buffer[64];
  snprintf(buffer, sizeof buffer, "http://%u.%u.%u.%u:%u", (unsigned) (ip & 0xff), (unsigned) ((ip >> 8) & 0xff),
           (unsigned) ((ip >> 16) & 0xff), (unsigned) ((ip >> 24) & 0xff), (unsigned) port);
  return buffer + path;
}

// "5 min", "3 h 20 min", for the out-of-date notice.
inline std::string age(uint32_t seconds) {
  uint32_t minutes = seconds / 60;
  if (minutes < 1)
    return "under 1 min";
  if (minutes < 60)
    return std::to_string(minutes) + " min";
  return std::to_string(minutes / 60) + " h " + std::to_string(minutes % 60) + " min";
}

}  // namespace eink_discovery
