// The HTTP/1.1 the display speaks to the server: a request written out, and a response taken apart. Every request asks
// for the connection to be closed after the answer, so a response is everything until the end of the stream.
//
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/).
#pragma once

#include <cstdlib>
#include <string>
#include <utility>
#include <vector>

namespace eink_http {

struct Response {
  int status = 0;       // 0: no response head at all
  std::string headers;  // the head, without its status line and the blank line after it
  std::string body;
};

inline std::string lower(std::string text) {
  for (char &c : text)
    if (c >= 'A' && c <= 'Z')
      c = static_cast<char>(c - 'A' + 'a');
  return text;
}

// A request with the body. `host` is the name the server is asked for (not the address connected to).
inline std::string request(const std::string &method, const std::string &path, const std::string &host,
                           const std::string &content_type, const std::string &body,
                           const std::vector<std::pair<std::string, std::string>> &headers = {}) {
  std::string out = method + " " + path + " HTTP/1.1\r\nHost: " + host + "\r\nConnection: close\r\n";
  if (!content_type.empty())
    out += "Content-Type: " + content_type + "\r\n";
  for (const auto &header : headers)
    out += header.first + ": " + header.second + "\r\n";
  out += "Content-Length: " + std::to_string(body.size()) + "\r\n\r\n";
  return out + body;
}

// The path and query of a URL, which is all that is sent to the server whatever its scheme and host say: the address is
// the one mDNS found and the name checked is the fixed one. "https://anything/image?x=1" is "/image?x=1"; a URL with no
// path is "/".
inline std::string path_of(const std::string &url) {
  const size_t scheme = url.find("://");
  const size_t start = url.find('/', scheme == std::string::npos ? 0 : scheme + 3);
  return start == std::string::npos ? "/" : url.substr(start);
}

// The value of a header (the name in any case), or "" if it is absent. `headers` is Response::headers.
inline std::string header(const std::string &headers, const std::string &name) {
  const std::string haystack = "\r\n" + lower(headers), needle = "\r\n" + lower(name) + ":";
  const size_t at = haystack.find(needle);
  if (at == std::string::npos)
    return "";
  const size_t start = at + needle.size() - 2;  // an index into `headers`: the haystack has two bytes more at the front
  const size_t end = headers.find("\r\n", start);
  std::string value = headers.substr(start, end == std::string::npos ? std::string::npos : end - start);
  while (!value.empty() && (value[0] == ' ' || value[0] == '\t'))
    value.erase(0, 1);
  while (!value.empty() && (value.back() == ' ' || value.back() == '\t'))
    value.pop_back();
  return value;
}

// Chunked transfer coding undone. A body cut short gives what arrived of it whole.
inline std::string unchunk(const std::string &raw) {
  std::string out;
  size_t at = 0;
  while (at < raw.size()) {
    const size_t eol = raw.find("\r\n", at);
    if (eol == std::string::npos)
      break;
    const size_t size = std::strtoul(raw.substr(at, eol - at).c_str(), nullptr, 16);
    if (size == 0 || eol + 2 + size > raw.size())
      break;
    out += raw.substr(eol + 2, size);
    at = eol + 2 + size + 2;
  }
  return out;
}

// Everything the server sent, taken apart. `status` is 0 if there is no complete head.
inline Response parse(const std::string &raw) {
  Response r;
  const size_t split = raw.find("\r\n\r\n");
  if (split == std::string::npos || raw.compare(0, 5, "HTTP/") != 0)
    return r;
  const std::string head = raw.substr(0, split);
  const size_t space = head.find(' ');
  if (space == std::string::npos)
    return r;
  const size_t line_end = head.find("\r\n");
  r.status = std::atoi(head.c_str() + space + 1);
  r.headers = line_end == std::string::npos ? "" : head.substr(line_end);
  r.body = raw.substr(split + 4);
  if (lower(header(r.headers, "transfer-encoding")).find("chunked") != std::string::npos)
    r.body = unchunk(r.body);
  return r;
}

}  // namespace eink_http
