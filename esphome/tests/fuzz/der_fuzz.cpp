// The DER reader (core/der.h): the CMS message the root arrives in, on the connection that verifies nothing, and the
// building blocks under it.
#include <algorithm>
#include <vector>

#include "fuzz.h"

#include "home_display/core/der.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  const home_display_der::Bytes input(data, data + size);

  // Whatever is found is a piece of what was given.
  for (const home_display_der::Bytes &cert : home_display_der::certificates_in(input)) {
    FUZZ_REQUIRE(cert.size() >= 2 && cert.size() <= input.size());
    FUZZ_REQUIRE(std::search(input.begin(), input.end(), cert.begin(), cert.end()) != input.end());
  }

  // An element read at any offset stays inside the buffer.
  for (size_t at = 0; at < std::min<size_t>(size, 48); at++) {
    const home_display_der::Tlv t = home_display_der::read(data + at, size - at);
    if (!t.ok)
      continue;
    FUZZ_REQUIRE(t.total <= size - at);
    FUZZ_REQUIRE(t.length <= t.total && t.total - t.length >= 2);
    FUZZ_REQUIRE(t.value >= data + at && t.value + t.length <= data + size);
  }

  // What is written is read back: a tag and a value, the way a request is built.
  if (size > 0 && size < 70000) {
    const uint8_t tag = data[0];
    const home_display_der::Bytes value(data + 1, data + size);
    const home_display_der::Bytes written = home_display_der::tlv(tag, value);
    const home_display_der::Tlv t = home_display_der::read(written.data(), written.size());
    FUZZ_REQUIRE(t.ok && t.tag == tag && t.total == written.size());
    FUZZ_REQUIRE(home_display_der::Bytes(t.value, t.value + t.length) == value);
  }
  return 0;
}
