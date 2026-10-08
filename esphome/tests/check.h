// A very small test harness, so the tests need nothing installed: `TEST(name) { ... }` registers a test, and
// CHECK / CHECK_EQ record a failure with its line and carry on. `run_all()` returns the number of failures.
#pragma once

#include <cstdio>
#include <functional>
#include <sstream>
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

template <typename T> std::string show(const T &value) {
  std::ostringstream out;
  out << value;
  return out.str();
}
inline std::string show(const char *value) { return value == nullptr ? "(null)" : std::string("\"") + value + "\""; }
inline std::string show(const std::string &value) { return "\"" + value + "\""; }
inline std::string show(bool value) { return value ? "true" : "false"; }
inline std::string show(uint8_t value) { return std::to_string((unsigned) value); }

inline int run_all() {
  for (const Case &c : cases()) {
    const int before = failures();
    c.body();
    std::printf("%s %s\n", failures() == before ? "ok  " : "FAIL", c.name);
  }
  std::printf("%zu tests, %d failed checks\n", cases().size(), failures());
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
