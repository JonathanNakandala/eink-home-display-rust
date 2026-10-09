#include <string>

#include "check.h"
#include "../eink_body.h"

using namespace eink_body;

// All of `raw` in pieces of `step` bytes; returns the body and leaves the response for inspection.
static std::string run(Response &r, const std::string &raw, size_t step, bool close = true) {
  std::string body;
  for (size_t at = 0; at < raw.size();) {
    const size_t n = std::min(step, raw.size() - at);
    const size_t used = r.feed(reinterpret_cast<const uint8_t *>(raw.data()) + at, n, body);
    at += used;
    if (used < n)
      break;  // ended or failed: the rest belongs to nothing
  }
  if (close)
    r.closed();
  return body;
}

static const std::string LENGTH = "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 11\r\n\r\nhello world";

TEST(a_response_with_a_length_comes_apart_whatever_the_pieces_are) {
  for (size_t step : {1u, 2u, 3u, 7u, 16u, 1000u}) {
    Response r;
    CHECK_EQ(run(r, LENGTH, step), "hello world");
    CHECK(r.state() == State::DONE);
    CHECK_EQ(r.status(), 200);
    CHECK_EQ(r.content_length(), (size_t) 11);
    CHECK(r.has_length());
    CHECK_EQ(r.header("content-type"), "image/png");
    CHECK_EQ(r.header("Content-Type"), "image/png");  // looked up in any case
    CHECK_EQ(r.body_bytes(), (size_t) 11);
  }
}

TEST(bytes_after_the_end_of_the_body_are_not_taken) {
  Response r;
  std::string body;
  const std::string raw = LENGTH + "EXTRA";
  const size_t used = r.feed(reinterpret_cast<const uint8_t *>(raw.data()), raw.size(), body);
  CHECK_EQ(used, LENGTH.size());
  CHECK_EQ(body, "hello world");
  CHECK(r.state() == State::DONE);
}

TEST(a_body_cut_short_by_the_connection_closing_is_a_failure) {
  Response r;
  run(r, "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello", 4);
  CHECK(r.state() == State::FAILED);
  Response head;
  run(head, "HTTP/1.1 200 OK\r\nContent", 4);
  CHECK(head.state() == State::FAILED);
}

TEST(a_zero_length_body_is_done_at_the_end_of_the_head) {
  Response r;
  run(r, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", 5, false);
  CHECK(r.state() == State::DONE);
}

TEST(a_chunked_body_is_put_together_whatever_the_pieces_are) {
  const std::string raw =
      "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n "
      "world\r\nA\r\n0123456789\r\n0\r\n\r\n";
  for (size_t step : {1u, 2u, 3u, 5u, 11u, 1000u}) {
    Response r;
    CHECK_EQ(run(r, raw, step, false), "hello world0123456789");
    CHECK(r.state() == State::DONE);
    CHECK(r.framing() == Framing::CHUNKED);
    CHECK(!r.has_length());
  }
}

TEST(a_chunked_body_with_a_trailer_ends_at_the_empty_line) {
  Response r;
  const std::string raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nX-Sum: 1\r\n\r\n";
  CHECK_EQ(run(r, raw, 2, false), "abc");
  CHECK(r.state() == State::DONE);
}

TEST(a_malformed_chunked_body_fails) {
  for (const char *bad : {"zz\r\n", "\r\n", "3\r\nabcXX", "3\r\nabc\r\nQ", "3\rX"}) {
    Response r;
    run(r, std::string("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n") + bad, 3, false);
    CHECK(r.state() == State::FAILED);
  }
}

TEST(a_body_that_runs_to_the_close_is_complete_when_the_connection_closes) {
  Response r;
  CHECK_EQ(run(r, "HTTP/1.0 200 OK\r\n\r\nall of it", 4), "all of it");
  CHECK(r.state() == State::DONE);
  CHECK(r.framing() == Framing::CLOSE);
}

TEST(replies_that_have_no_body_have_none) {
  for (const char *status : {"204 No Content", "304 Not Modified", "100 Continue"}) {
    Response r;
    run(r, std::string("HTTP/1.1 ") + status + "\r\nContent-Length: 50\r\n\r\n", 6, false);
    CHECK(r.state() == State::DONE);
    CHECK(r.bodyless());
  }
  Response r;
  run(r, "HTTP/1.1 304 Not Modified\r\nETag: \"x\"\r\n\r\n", 6, false);
  CHECK_EQ(r.status(), 304);
  CHECK_EQ(r.header("etag"), "\"x\"");
}

TEST(a_head_or_a_framing_this_cannot_follow_fails) {
  for (const char *bad : {
           "garbage\r\n\r\n",
           "HTTP/1.1 abc OK\r\n\r\n",
           "HTTP/1.1 20 OK\r\n\r\n",
           "HTTP/1.1 999 OK\r\n\r\n",
           "HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
           "HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n",
           "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
           "HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999\r\n\r\n",
           "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
           "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n",
       }) {
    Response r;
    run(r, bad, 4, false);
    CHECK(r.state() == State::FAILED);
  }
}

TEST(a_head_that_never_ends_is_refused_at_a_limit) {
  Response r;
  std::string endless = "HTTP/1.1 200 OK\r\n";
  while (endless.size() < Response::MAX_HEAD + 100)
    endless += "X-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n";
  run(r, endless, 50, false);
  CHECK(r.state() == State::FAILED);
}

TEST(header_values_are_trimmed_and_names_lower_cased) {
  Response r;
  run(r, "HTTP/1.1 202 Accepted\r\nRetry-After:   300  \r\nX-Two:\t2\r\nContent-Length: 0\r\n\r\n", 7, false);
  CHECK_EQ(r.header("retry-after"), "300");
  CHECK_EQ(r.header("RETRY-AFTER"), "300");
  CHECK_EQ(r.header("x-two"), "2");
  CHECK_EQ(r.header("absent"), "");
  CHECK_EQ(r.status(), 202);
}

TEST(a_large_body_in_small_pieces_is_passed_on_without_being_held) {
  const size_t total = 300000;
  const std::string head = "HTTP/1.1 200 OK\r\nContent-Length: " + std::to_string(total) + "\r\n\r\n";
  Response r;
  size_t seen = 0;
  std::string piece;
  r.feed(reinterpret_cast<const uint8_t *>(head.data()), head.size(), piece);
  for (size_t at = 0; at < total; at += 1400) {
    piece.clear();
    const std::string chunk(std::min<size_t>(1400, total - at), 'z');
    r.feed(reinterpret_cast<const uint8_t *>(chunk.data()), chunk.size(), piece);
    seen += piece.size();
  }
  CHECK_EQ(seen, total);
  CHECK(r.state() == State::DONE);
}

TEST(a_length_or_chunk_too_big_for_the_chip_is_refused_here_too) {
  // 2^32 would wrap to 0 in a 32-bit size_t, which is what the chip has; nothing that large is a plan or a picture.
  for (const char *bad : {"HTTP/1.1 200 OK\r\nContent-Length: 4294967296\r\n\r\n",
                          "HTTP/1.1 200 OK\r\nContent-Length: 2147483648\r\n\r\n",
                          "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100000000\r\n",
                          "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n80000000\r\n"}) {
    Response r;
    run(r, bad, 5, false);
    CHECK(r.state() == State::FAILED);
  }
  Response ok;
  run(ok, "HTTP/1.1 200 OK\r\nContent-Length: 2147483647\r\n\r\n", 5, false);
  CHECK(ok.state() == State::BODY);  // the largest accepted
  CHECK_EQ(ok.content_length(), (size_t) 2147483647);
}
