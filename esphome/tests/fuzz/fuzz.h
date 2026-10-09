// What a fuzz harness needs: a check that dies loudly, so the fuzzer keeps the input that broke it.
#pragma once

#include <cstdio>
#include <cstdlib>

#define FUZZ_REQUIRE(condition)                                                               \
  do {                                                                                        \
    if (!(condition)) {                                                                       \
      std::fprintf(stderr, "invariant broken: %s (%s:%d)\n", #condition, __FILE__, __LINE__); \
      std::abort();                                                                           \
    }                                                                                         \
  } while (0)
