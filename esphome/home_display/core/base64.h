// Base64 (RFC 4648, the standard alphabet with padding), for the bodies EST puts on the wire: a request and a
// certificate each travel as base64 of DER.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace home_display_base64 {

using Bytes = std::vector<uint8_t>;

inline std::string encode(const Bytes &data) {
  static const char alphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  std::string out;
  out.reserve((data.size() + 2) / 3 * 4);
  for (size_t i = 0; i < data.size(); i += 3) {
    const uint32_t a = data[i], b = i + 1 < data.size() ? data[i + 1] : 0, c = i + 2 < data.size() ? data[i + 2] : 0;
    const uint32_t n = (a << 16) | (b << 8) | c;
    out += alphabet[(n >> 18) & 63];
    out += alphabet[(n >> 12) & 63];
    out += i + 1 < data.size() ? alphabet[(n >> 6) & 63] : '=';
    out += i + 2 < data.size() ? alphabet[n & 63] : '=';
  }
  return out;
}

// Line breaks and spaces are skipped, as a MIME-wrapped body has them. Anything else outside the alphabet, padding
// anywhere but the end, or a length that cannot be base64 fails and leaves `out` empty.
inline bool decode(const std::string &text, Bytes &out) {
  out.clear();
  uint32_t buffer = 0;
  int bits = 0, padding = 0;
  for (char c : text) {
    if (c == '\r' || c == '\n' || c == ' ' || c == '\t')
      continue;
    int value;
    if (c >= 'A' && c <= 'Z')
      value = c - 'A';
    else if (c >= 'a' && c <= 'z')
      value = c - 'a' + 26;
    else if (c >= '0' && c <= '9')
      value = c - '0' + 52;
    else if (c == '+')
      value = 62;
    else if (c == '/')
      value = 63;
    else if (c == '=') {
      padding++;
      continue;
    } else {
      out.clear();
      return false;
    }
    if (padding != 0) {  // data after padding
      out.clear();
      return false;
    }
    buffer = (buffer << 6) | static_cast<uint32_t>(value);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push_back(static_cast<uint8_t>(buffer >> bits));
      buffer &= (1u << bits) - 1;
    }
  }
  // Whole quanta only: padding makes up what is missing, and no more than two characters of it.
  const size_t characters = (out.size() * 8 + bits) / 6 + padding;
  if (padding > 2 || characters % 4 != 0 || (bits != 0 && buffer != 0)) {
    out.clear();
    return false;
  }
  return true;
}

}  // namespace home_display_base64
