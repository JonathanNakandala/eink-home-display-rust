#include <string>

#include "check.h"
#include "eink/core/http.h"

using namespace eink_http;

TEST(a_request_names_the_host_asks_to_close_and_says_how_long_the_body_is) {
  const std::string r = request("POST", "/x", "eink-home-display.internal", "application/pkcs10", "abc");
  CHECK_EQ(r,
           "POST /x HTTP/1.1\r\nHost: eink-home-display.internal\r\nConnection: close\r\n"
           "Content-Type: application/pkcs10\r\nContent-Length: 3\r\n\r\nabc");
  CHECK_EQ(request("GET", "/y", "h", "", ""),
           "GET /y HTTP/1.1\r\nHost: h\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
}

TEST(a_response_with_a_length_is_taken_apart) {
  const Response r = parse("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello");
  CHECK_EQ(r.status, 200);
  CHECK_EQ(r.body, "hello");
  CHECK_EQ(header(r.headers, "Content-Type"), "text/plain");
  CHECK_EQ(header(r.headers, "content-length"), "5");
}

TEST(a_header_is_found_in_any_case_and_trimmed) {
  const Response r = parse("HTTP/1.1 202 Accepted\r\nRetry-After:   300  \r\nX-Other: 1\r\n\r\n");
  CHECK_EQ(r.status, 202);
  CHECK_EQ(header(r.headers, "retry-after"), "300");
  CHECK_EQ(header(r.headers, "RETRY-AFTER"), "300");
  CHECK_EQ(header(r.headers, "x-other"), "1");
  CHECK_EQ(header(r.headers, "missing"), "");
  CHECK_EQ(r.body, "");
}

TEST(a_header_whose_name_ends_the_same_is_not_mistaken_for_it) {
  const Response r = parse("HTTP/1.1 200 OK\r\nX-Retry-After: 9\r\nRetry-After: 4\r\n\r\n");
  CHECK_EQ(header(r.headers, "retry-after"), "4");
}

TEST(a_chunked_body_is_put_together) {
  const Response r =
      parse("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
  CHECK_EQ(r.body, "hello world");
}

TEST(a_chunked_body_cut_short_gives_the_chunks_that_arrived_whole) {
  CHECK_EQ(unchunk("5\r\nhello\r\n6\r\n wor"), "hello");
  CHECK_EQ(unchunk("5\r\nhel"), "");
  CHECK_EQ(unchunk(""), "");
  CHECK_EQ(unchunk("zz\r\n"), "");
}

TEST(no_complete_head_is_no_response) {
  CHECK_EQ(parse("").status, 0);
  CHECK_EQ(parse("HTTP/1.1 200 OK\r\ncontent-length: 5").status, 0);
  CHECK_EQ(parse("garbage\r\n\r\n").status, 0);
  CHECK_EQ(parse("HTTP/1.1\r\n\r\n").status, 0);
}

TEST(the_status_is_whatever_the_server_said) {
  CHECK_EQ(parse("HTTP/1.1 403 Forbidden\r\n\r\n").status, 403);
  CHECK_EQ(parse("HTTP/1.1 429 Too Many Requests\r\nretry-after: 5\r\n\r\nslow").status, 429);
}

TEST(only_the_path_and_query_of_a_url_are_sent) {
  CHECK_EQ(path_of("https://eink-home-display.internal/image?device=a"), "/image?device=a");
  CHECK_EQ(path_of("http://192.168.1.20:8443/plan"), "/plan");
  CHECK_EQ(path_of("/plan?have=1"), "/plan?have=1");
  CHECK_EQ(path_of("https://host"), "/");
  CHECK_EQ(path_of("https://host?x=1"), "/");
  CHECK_EQ(path_of(""), "/");
  CHECK_EQ(path_of("https://host/a/b/c.png"), "/a/b/c.png");
}

// Found by fuzzing (tests/fuzz/regressions/http): a chunk size too big to add to a position wrapped it, so the check
// that it fitted passed, and the rest of the reply was appended over and over, making a body many times the reply.
TEST(a_chunk_size_too_big_to_be_a_size_ends_the_body_and_makes_nothing) {
  using eink_http::unchunk;
  CHECK_EQ(unchunk("ffffffffffffffff\r\nx\r\n0\r\n\r\n"), "");
  CHECK_EQ(unchunk("5\r\nhello\r\nffffffffffffffff\r\nx\r\n0\r\n\r\n"), "hello");  // what came before is kept
  CHECK_EQ(unchunk("100000000\r\nx"), "");  // nine digits: more than a 32-bit size
  CHECK_EQ(unchunk("7fffffff\r\nx"), "");   // a size that fits and is not there
  CHECK_EQ(unchunk("-1\r\nx\r\n"), "");     // no sign, which strtoul would have read as the largest number
  CHECK_EQ(unchunk(" 5\r\nhello\r\n"), "");
  CHECK_EQ(unchunk("5;ext=1\r\nhello\r\n0\r\n\r\n"), "hello");  // an extension is allowed
  CHECK_EQ(unchunk("A\r\n0123456789\r\n0\r\n\r\n"), "0123456789");
}

TEST(a_body_that_unchunks_is_never_longer_than_what_it_came_from) {
  std::string raw = "1\r\na\r\n";
  for (int i = 0; i < 50; i++)
    raw += "ffffffffffffffff\r\nline\r\n";
  CHECK(eink_http::unchunk(raw).size() <= raw.size());
}
