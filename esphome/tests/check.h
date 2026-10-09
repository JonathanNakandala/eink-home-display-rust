// A very small test harness, so the tests need nothing installed: `TEST(name) { ... }` registers a test, and
// CHECK / CHECK_EQ record a failure with its line and carry on. `run_all()` returns the number of failures.
#pragma once

#include <cstdio>
#include <functional>
#include <sstream>
#include <type_traits>
#include <string>
#include <vector>

namespace check {

struct Case {
  const char *name;
  std::function<void()> body;
};

inline std::vector<Case> &cases() {
  static std::vector<Case> all;
  return all;
}

inline int &failures() {
  static int count = 0;
  return count;
}

struct Register {
  Register(const char *name, std::function<void()> body) { cases().push_back({name, std::move(body)}); }
};

template <typename T> std::enable_if_t<!std::is_enum_v<T>, std::string> show(const T &value) {
  std::ostringstream out;
  out << value;
  return out.str();
}
// An enum class has no <<; its number is what a failing check can say.
template <typename T> std::enable_if_t<std::is_enum_v<T>, std::string> show(const T &value) {
  return std::to_string((long long) value);
}
inline std::string show(const char *value) { return value == nullptr ? "(null)" : std::string("\"") + value + "\""; }
inline std::string show(const std::string &value) { return "\"" + value + "\""; }
// Bytes, as hex.
inline std::string show(const std::vector<uint8_t> &value) {
  static const char digits[] = "0123456789abcdef";
  std::string out = "bytes[";
  for (uint8_t byte : value) {
    out += digits[byte >> 4];
    out += digits[byte & 15];
  }
  return out + "]";
}
inline std::string show(bool value) { return value ? "true" : "false"; }
inline std::string show(uint8_t value) { return std::to_string((unsigned) value); }

// With no arguments, runs every test. `--list` prints their names, one to a line, and a name runs just that test: for
// tests that each need a fresh server (host/).
inline int run_all(int argc = 0, char **argv = nullptr) {
  if (argc > 1 && std::string(argv[1]) == "--list") {
    for (const Case &c : cases())
      std::printf("%s\n", c.name);
    return 0;
  }
  size_t ran = 0;
  for (const Case &c : cases()) {
    if (argc > 1 && std::string(argv[1]) != c.name)
      continue;
    const int before = failures();
    c.body();
    ran++;
    std::printf("%s %s\n", failures() == before ? "ok  " : "FAIL", c.name);
  }
  if (argc > 1 && ran == 0) {
    std::printf("no test is called %s\n", argv[1]);
    return 2;
  }
  std::printf("%zu tests, %d failed checks\n", ran, failures());
  return failures() == 0 ? 0 : 1;
}

}  // namespace check

#define TEST(name)                                            \
  static void test_##name();                                  \
  static check::Register register_##name(#name, test_##name); \
  static void test_##name()

#define CHECK(condition)                                                          \
  do {                                                                            \
    if (!(condition)) {                                                           \
      std::printf("  %s:%d: CHECK(%s) failed\n", __FILE__, __LINE__, #condition); \
      check::failures()++;                                                        \
    }                                                                             \
  } while (0)

#define CHECK_EQ(actual, expected)                                                                \
  do {                                                                                            \
    const auto a_ = (actual);                                                                     \
    const auto e_ = (expected);                                                                   \
    if (!(a_ == e_)) {                                                                            \
      std::printf("  %s:%d: %s\n    got      %s\n    expected %s\n", __FILE__, __LINE__, #actual, \
                  check::show(a_).c_str(), check::show(e_).c_str());                              \
      check::failures()++;                                                                        \
    }                                                                                             \
  } while (0)
