// The HTTP response parser (core/body.h) on whatever bytes a server sends, delivered in whatever pieces. The first byte
// is how the network cuts it up (and whether the connection then closes); the rest is the response.
//
// Beyond not crashing: how the bytes are cut must make no difference to what comes out, the body handed on must be what
// the framing says, and a response that has ended takes no more.
#include <algorithm>
#include <string>
#include <vector>

#include "fuzz.h"

#include "eink/core/body.h"

namespace {

struct Result {
  eink_body::State state;
  int status;
  eink_body::Framing framing;
  size_t content_length;
  std::string body;
  size_t body_bytes;
  std::vector<std::pair<std::string, std::string>> headers;
};

Result follow(const uint8_t *in, size_t length, size_t piece, bool closes) {
  eink_body::Response response;
  std::string body;
  for (size_t at = 0; at < length;) {
    const size_t take = std::min(piece, length - at);
    const size_t used = response.feed(in + at, take, body);
    FUZZ_REQUIRE(used <= take);
    at += take;
    if (used < take) {  // it ended (done or failed) and the rest belongs to nothing
      FUZZ_REQUIRE(response.state() == eink_body::State::DONE || response.state() == eink_body::State::FAILED);
      break;
    }
  }
  // An ended response takes no more.
  if (response.state() == eink_body::State::DONE || response.state() == eink_body::State::FAILED) {
    const uint8_t more[] = {'x', 'y'};
    std::string extra;
    FUZZ_REQUIRE(response.feed(more, sizeof more, extra) == 0);
    FUZZ_REQUIRE(extra.empty());
  }
  if (closes)
    response.closed();
  FUZZ_REQUIRE(body.size() == response.body_bytes());
  if (response.has_length()) {
    FUZZ_REQUIRE(body.size() <= response.content_length());
    if (response.state() == eink_body::State::DONE)
      FUZZ_REQUIRE(body.size() == response.content_length());
  }
  if (closes)
    FUZZ_REQUIRE(response.state() == eink_body::State::DONE || response.state() == eink_body::State::FAILED);
  Result result{response.state(),
                response.status(),
                response.framing(),
                response.content_length(),
                body,
                response.body_bytes(),
                {}};
  for (const auto &h : response.headers())
    result.headers.emplace_back(h.name, h.value);
  return result;
}

}  // namespace

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  if (size == 0)
    return 0;
  const size_t piece = 1 + data[0] % 61;
  const bool closes = (data[0] & 0x80) != 0;
  const uint8_t *in = data + 1;
  const size_t length = size - 1;

  const Result whole = follow(in, length, std::max<size_t>(length, 1), closes);
  const Result cut = follow(in, length, piece, closes);
  const Result bytewise = follow(in, length, 1, closes);
  for (const Result *other : {&cut, &bytewise}) {
    FUZZ_REQUIRE(whole.state == other->state);
    FUZZ_REQUIRE(whole.status == other->status);
    FUZZ_REQUIRE(whole.framing == other->framing);
    FUZZ_REQUIRE(whole.content_length == other->content_length);
    FUZZ_REQUIRE(whole.body == other->body);
    FUZZ_REQUIRE(whole.headers == other->headers);
  }
  return 0;
}
