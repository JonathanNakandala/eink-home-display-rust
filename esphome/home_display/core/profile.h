// What the display says it is when it asks to join: its model, its panel, the image formats it can decode and the
// firmware it runs. It goes in the certificate request, as an extension under a private OID in the `extensionRequest`
// attribute (RFC 2985), signed with the rest of the request, so the owner sees it before approving and the server can
// tell what is asking.
//
// It is the display's own word and the pairing code does not cover it, so the server treats it as text for the owner:
// bounded, and plain characters. The same limits are here, so the display never sends what the server would drop. A
// profile that does not pass them is not sent, and the request goes on without; a profile can never keep a display from
// joining.
//
// Pure calculation (tests/core/); the server reads it in src/adapters/certificate_authority/request.rs, and
// testdata/contract/ holds requests both sides' tests read.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include "home_display/core/der.h"

namespace home_display_profile {

using Bytes = home_display_der::Bytes;

// The version of the structure below. A server that reads a newer one than it knows leaves it out.
constexpr uint32_t VERSION = 1;

struct Profile {
  std::string model;     // what the display is: "reTerminal E1003"
  std::string firmware;  // the release of its firmware
  uint32_t width = 0;    // the panel in pixels
  uint32_t height = 0;
  uint32_t levels = 0;               // how many greys it shows
  std::vector<std::string> formats;  // the image formats it can decode: "bmp", "png", "qoi"
};

namespace limits {
constexpr size_t MODEL_CHARS = 32;
constexpr size_t FIRMWARE_CHARS = 32;
constexpr size_t FORMAT_CHARS = 16;
constexpr size_t MAX_FORMATS = 8;
constexpr uint32_t MAX_PIXELS = 20000;
constexpr uint32_t MIN_LEVELS = 2;
constexpr uint32_t MAX_LEVELS = 256;
}  // namespace limits

inline bool model_character(char c) {
  return (c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == ' ' || c == '.' ||
         c == '_' || c == '-' || c == ':' || c == '+';
}

inline bool firmware_character(char c) {
  return (c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '.' || c == '_' || c == '-';
}

inline bool format_character(char c) {
  return (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '.' || c == '+' || c == '-';
}

inline bool all_of(const std::string &text, bool (*allowed)(char)) {
  for (const char c : text)
    if (!allowed(c))
      return false;
  return true;
}

// Whether the server will keep this profile: the same rules as its (domain/models/profile.rs).
inline bool usable(const Profile &p) {
  if (p.model.empty() || p.model.size() > limits::MODEL_CHARS || !all_of(p.model, model_character))
    return false;
  if (p.firmware.empty() || p.firmware.size() > limits::FIRMWARE_CHARS || !all_of(p.firmware, firmware_character))
    return false;
  if (p.width < 1 || p.width > limits::MAX_PIXELS || p.height < 1 || p.height > limits::MAX_PIXELS)
    return false;
  if (p.levels < limits::MIN_LEVELS || p.levels > limits::MAX_LEVELS)
    return false;
  if (p.formats.size() > limits::MAX_FORMATS)
    return false;
  for (const std::string &format : p.formats)
    if (format.empty() || format.size() > limits::FORMAT_CHARS || !all_of(format, format_character))
      return false;
  return true;
}

// The formats in an `Accept` header, as the short names the profile uses: "image/bmp, image/png;q=0.5, */*" is {"bmp",
// "png"}. Anything that is not a plain `image/<name>` (a wildcard, another type) is left out, and a name twice is said
// once.
inline std::vector<std::string> formats_from_accept(const std::string &accept) {
  std::vector<std::string> out;
  size_t at = 0;
  while (at <= accept.size()) {
    size_t end = accept.find(',', at);
    if (end == std::string::npos)
      end = accept.size();
    std::string item = accept.substr(at, end - at);
    at = end + 1;
    const size_t parameters = item.find(';');
    if (parameters != std::string::npos)
      item.erase(parameters);
    size_t a = 0, b = item.size();
    while (a < b && (item[a] == ' ' || item[a] == '\t'))
      a++;
    while (b > a && (item[b - 1] == ' ' || item[b - 1] == '\t'))
      b--;
    item = item.substr(a, b - a);
    const std::string prefix = "image/";
    if (item.compare(0, prefix.size(), prefix) != 0)
      continue;
    std::string name = item.substr(prefix.size());
    for (char &c : name)
      if (c >= 'A' && c <= 'Z')
        c = static_cast<char>(c - 'A' + 'a');
    if (name.empty() || name == "*")
      continue;
    bool seen = false;
    for (const std::string &have : out)
      seen = seen || have == name;
    if (!seen)
      out.push_back(name);
  }
  return out;
}

inline Bytes integer(uint32_t value) {
  Bytes content;
  for (int shift = 24; shift >= 0; shift -= 8)
    content.push_back(static_cast<uint8_t>(value >> shift));
  while (content.size() > 1 && content[0] == 0 && content[1] < 0x80)
    content.erase(content.begin());
  if (content[0] >= 0x80)
    content.insert(content.begin(), 0x00);  // a positive number, not a negative one
  return home_display_der::tlv(0x02, content);
}

inline Bytes text(const std::string &value) { return home_display_der::tlv(0x0C, Bytes(value.begin(), value.end())); }

// The profile as DER:
//   SEQUENCE { INTEGER version, UTF8String model, INTEGER width, INTEGER height, INTEGER levels,
//              SEQUENCE OF UTF8String formats, UTF8String firmware }
// Empty if it is not usable, so nothing the server would drop is sent.
inline Bytes encode(const Profile &p) {
  if (!usable(p))
    return {};
  Bytes formats;
  for (const std::string &format : p.formats) {
    const Bytes one = text(format);
    formats.insert(formats.end(), one.begin(), one.end());
  }
  return home_display_der::tlv(
      0x30, home_display_der::concat({integer(VERSION), text(p.model), integer(p.width), integer(p.height),
                                      integer(p.levels), home_display_der::tlv(0x30, formats), text(p.firmware)}));
}

}  // namespace home_display_profile
