// How an attempt to reach the server over TLS came out, without any of what it took: a name for the result that the
// decisions (eink_secure_wake.h) can use and be tested with on a computer, where mbedTLS is not part of that test.
#pragma once

#include <cstdint>

namespace eink_link {

enum class Start : uint8_t {
  OK,           // the head of the answer has been read
  UNREACHABLE,  // no connection, no handshake, no answer
  REFUSED,      // a certificate was turned down (the server's by us, or ours by it)
  BAD_REPLY,    // an answer that is not an HTTP response we can follow
};

}  // namespace eink_link
