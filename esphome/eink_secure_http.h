// The server over TLS as ESPHome's own HTTP component, so `online_image` downloads the picture through it unchanged.
//
// `online_image` is `Parented` to an `HttpRequestComponent` and asks it for a `HttpContainer` that it reads the body
// from. The stock component cannot do what the display needs (it checks a certificate against the address, takes a CA
// at build time, and has no client certificate), so this one is pointed at instead:
// `dashboard->set_parent(&eink_secure::http())`. It reads where the server is, and what to show it, from eink_secure.h.
//
// Only the chip's build has this (it needs ESPHome's headers); the TLS and the streaming under it are eink_stream.h,
// which is tested on a computer.
#pragma once

#include <memory>
#include <string>
#include <vector>

#include "esphome/components/http_request/http_request.h"
#include "esphome/components/watchdog/watchdog.h"
#include "esphome/core/application.h"

#include "eink_http.h"
#include "eink_secure.h"

namespace eink_secure {

class Container : public esphome::http_request::HttpContainer {
 public:
  eink_stream::Stream stream;

  // The body, as it comes. 0 is the end of it (see `is_read_complete`), and a negative number is a failure.
  int read(uint8_t *buf, size_t max_len) override {
    const uint32_t start = millis();
    esphome::watchdog::WatchdogManager wdm(this->parent_->get_watchdog_timeout());
    App.feed_wdt();
    const int n = stream.read(buf, max_len);
    App.feed_wdt();
    this->duration_ms += millis() - start;
    if (n > 0)
      this->bytes_read_ += static_cast<size_t>(n);
    return n;
  }

  bool is_read_complete() const override { return stream.finished(); }

  // Keeps the response headers the caller asked for, by the lower-case names it gave.
  void collect(const std::vector<std::string> &lower_case_names) {
    for (const std::string &name : lower_case_names)
      for (const eink_body::Header &h : stream.headers())
        if (h.name == name)
          this->response_headers_.push_back({h.name, h.value});
  }

  void end() override { stream.close(); }
};

class Http final : public esphome::http_request::HttpRequestComponent {
 public:
  void dump_config() override {}

 protected:
  std::shared_ptr<esphome::http_request::HttpContainer> perform(
      const std::string &url, const std::string &method, const std::string &body,
      const std::vector<esphome::http_request::Header> &request_headers,
      const std::vector<std::string> &lower_case_collect_headers) override {
    Context &c = context();
    if (!c.ready)
      return nullptr;
    esphome::watchdog::WatchdogManager wdm(this->get_watchdog_timeout());
    const uint32_t start = millis();

    auto container = std::make_shared<Container>();
    container->set_parent(this);
    container->set_secure(true);

    eink_stream::Headers headers;
    for (const auto &h : request_headers)
      headers.push_back({h.name, h.value});
    eink_stream::Peer peer = c.peer;
    peer.timeout_ms = static_cast<int>(this->get_timeout());
    App.feed_wdt();
    c.last = container->stream.start(peer, method, eink_http::path_of(url), headers,
                                     body.empty() ? "" : "application/octet-stream", body);
    App.feed_wdt();
    if (c.last != eink_stream::Stream::Start::OK)
      return nullptr;

    container->status_code = container->stream.status();
    container->content_length = container->stream.content_length();
    container->set_chunked(!container->stream.has_length());
    container->collect(lower_case_collect_headers);
    container->duration_ms = millis() - start;
    return container;
  }
};

// The one instance. Not registered as a component: it has no setup or loop, and is only pointed at.
inline Http &http() {
  static Http instance;
  return instance;
}

}  // namespace eink_secure
