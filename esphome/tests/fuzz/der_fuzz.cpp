// The DER reader (core/der.h): the CMS message the root arrives in, on the connection that verifies nothing, and the
// building blocks under it.
#include <algorithm>
#include <vector>

#include "fuzz.h"

#include "eink/core/der.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  const eink_der::Bytes input(data, data + size);

  // Whatever is found is a piece of what was given.
  for (const eink_der::Bytes &cert : eink_der::certificates_in(input)) {
    FUZZ_REQUIRE(cert.size() >= 2 && cert.size() <= input.size());
    FUZZ_REQUIRE(std::search(input.begin(), input.end(), cert.begin(), cert.end()) != input.end());
  }

  // An element read at any offset stays inside the buffer.
  for (size_t at = 0; at < std::min<size_t>(size, 48); at++) {
    const eink_der::Tlv t = eink_der::read(data + at, size - at);
    if (!t.ok)
      continue;
    FUZZ_REQUIRE(t.total <= size - at);
    FUZZ_REQUIRE(t.length <= t.total && t.total - t.length >= 2);
    FUZZ_REQUIRE(t.value >= data + at && t.value + t.length <= data + size);
  }

  // What is written is read back: a tag and a value, the way a request is built.
  if (size > 0 && size < 70000) {
    const uint8_t tag = data[0];
    const eink_der::Bytes value(data + 1, data + size);
    const eink_der::Bytes written = eink_der::tlv(tag, value);
    const eink_der::Tlv t = eink_der::read(written.data(), written.size());
    FUZZ_REQUIRE(t.ok && t.tag == tag && t.total == written.size());
    FUZZ_REQUIRE(eink_der::Bytes(t.value, t.value + t.length) == value);
  }
  return 0;
}
