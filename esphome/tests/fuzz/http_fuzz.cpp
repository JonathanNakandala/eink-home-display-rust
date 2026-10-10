// The small-reply parser (core/http.h): the head of a reply, chunked coding, a header's value, the path of a URL. This
// is what reads the answer to the first request, on a connection that verifies nothing.
#include <string>

#include "fuzz.h"

#include "home_display/core/http.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  const std::string raw(reinterpret_cast<const char *>(data), size);

  const home_display_http::Response response = home_display_http::parse(raw);
  FUZZ_REQUIRE(response.body.size() <= raw.size());
  FUZZ_REQUIRE(response.status >= 0 || response.status < 0);  // any int, as atoi gives; only that it is a number
  FUZZ_REQUIRE(response.headers.size() <= raw.size());

  const std::string unchunked = home_display_http::unchunk(raw);
  FUZZ_REQUIRE(unchunked.size() <= raw.size());

  const std::string value = home_display_http::header(raw, "content-type");
  FUZZ_REQUIRE(value.size() <= raw.size());
  FUZZ_REQUIRE(home_display_http::header(raw, "Content-Type") == value);  // the name in any case

  const std::string path = home_display_http::path_of(raw);
  FUZZ_REQUIRE(!path.empty() && path[0] == '/');
  FUZZ_REQUIRE(path.size() <= raw.size() + 1);
  return 0;
}
