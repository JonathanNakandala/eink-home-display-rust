// Base64 (core/base64.h) on the body of a reply, and on what is built to send.
#include <string>

#include "fuzz.h"

#include "eink/core/base64.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  // Anything as text: when it decodes, what came out encodes to something that decodes to the same.
  const std::string text(reinterpret_cast<const char *>(data), size);
  eink_base64::Bytes decoded;
  if (eink_base64::decode(text, decoded)) {
    FUZZ_REQUIRE(decoded.size() <= size);
    eink_base64::Bytes again;
    FUZZ_REQUIRE(eink_base64::decode(eink_base64::encode(decoded), again));
    FUZZ_REQUIRE(again == decoded);
  } else {
    FUZZ_REQUIRE(decoded.empty());  // a failure leaves nothing
  }

  // Anything as bytes: it comes back as it went.
  const eink_base64::Bytes bytes(data, data + size);
  eink_base64::Bytes back;
  const std::string encoded = eink_base64::encode(bytes);
  FUZZ_REQUIRE(encoded.size() % 4 == 0);
  FUZZ_REQUIRE(eink_base64::decode(encoded, back));
  FUZZ_REQUIRE(back == bytes);
  return 0;
}
