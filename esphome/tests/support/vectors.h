// Reads the shared test vectors (testdata/contract/, which the server's tests read too): `[case]` lines, then `key:
// value` lines, `#` for comments. See testdata/contract/README.md. A file that is missing or has no case aborts the
// test, so the checks cannot pass by finding nothing.
#pragma once

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <string>
#include <utility>
#include <vector>

#ifndef VECTORS_DIR
#error "VECTORS_DIR is the path of testdata/contract; the Makefile gives it"
#endif

namespace vectors {

struct Case {
  std::string name;
  std::vector<std::pair<std::string, std::string>> fields;

  bool has(const std::string &key) const {
    for (const auto &f : fields)
      if (f.first == key)
        return true;
    return false;
  }
  // The value of `key`, or `otherwise` if the case has none.
  std::string get(const std::string &key, const std::string &otherwise = "") const {
    for (const auto &f : fields)
      if (f.first == key)
        return f.second;
    return otherwise;
  }
};

inline std::string trim(const std::string &text) {
  size_t a = 0, b = text.size();
  while (a < b && (text[a] == ' ' || text[a] == '\t' || text[a] == '\r'))
    a++;
  while (b > a && (text[b - 1] == ' ' || text[b - 1] == '\t' || text[b - 1] == '\r'))
    b--;
  return text.substr(a, b - a);
}

inline std::vector<Case> load(const std::string &file) {
  const std::string path = std::string(VECTORS_DIR) + "/" + file;
  std::ifstream in(path);
  if (!in) {
    std::fprintf(stderr, "vectors: cannot read %s\n", path.c_str());
    std::abort();
  }
  std::vector<Case> cases;
  std::string line;
  while (std::getline(in, line)) {
    line = trim(line);
    if (line.empty() || line[0] == '#')
      continue;
    if (line[0] == '[' && line.back() == ']') {
      cases.push_back(Case{line.substr(1, line.size() - 2), {}});
      continue;
    }
    const size_t colon = line.find(':');
    if (colon == std::string::npos || cases.empty()) {
      std::fprintf(stderr, "vectors: %s: cannot read the line \"%s\"\n", path.c_str(), line.c_str());
      std::abort();
    }
    cases.back().fields.emplace_back(trim(line.substr(0, colon)), trim(line.substr(colon + 1)));
  }
  if (cases.empty()) {
    std::fprintf(stderr, "vectors: %s has no case\n", path.c_str());
    std::abort();
  }
  return cases;
}

inline std::vector<uint8_t> from_hex(const std::string &text) {
  std::vector<uint8_t> out;
  for (size_t i = 0; i + 1 < text.size(); i += 2)
    out.push_back(static_cast<uint8_t>(std::strtoul(text.substr(i, 2).c_str(), nullptr, 16)));
  return out;
}

inline std::string to_hex(const std::vector<uint8_t> &bytes) {
  std::string out;
  char buffer[3];
  for (uint8_t b : bytes) {
    std::snprintf(buffer, sizeof buffer, "%02x", b);
    out += buffer;
  }
  return out;
}

inline std::vector<std::string> split(const std::string &text, char separator) {
  std::vector<std::string> out;
  size_t at = 0;
  for (;;) {
    const size_t next = text.find(separator, at);
    out.push_back(text.substr(at, next == std::string::npos ? std::string::npos : next - at));
    if (next == std::string::npos)
      return out;
    at = next + 1;
  }
}

}  // namespace vectors
