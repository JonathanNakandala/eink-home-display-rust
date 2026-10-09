#include <string>

#include "check.h"
#include "eink/core/base64.h"

using namespace eink_base64;

static Bytes bytes(const std::string &text) { return Bytes(text.begin(), text.end()); }

// RFC 4648 section 10.
TEST(the_rfc_examples_encode) {
  CHECK_EQ(encode(bytes("")), "");
  CHECK_EQ(encode(bytes("f")), "Zg==");
  CHECK_EQ(encode(bytes("fo")), "Zm8=");
  CHECK_EQ(encode(bytes("foo")), "Zm9v");
  CHECK_EQ(encode(bytes("foob")), "Zm9vYg==");
  CHECK_EQ(encode(bytes("fooba")), "Zm9vYmE=");
  CHECK_EQ(encode(bytes("foobar")), "Zm9vYmFy");
}

TEST(the_rfc_examples_decode) {
  Bytes out;
  for (const char *text : {"", "f", "fo", "foo", "foob", "fooba", "foobar"}) {
    CHECK(decode(encode(bytes(text)), out));
    CHECK(out == bytes(text));
  }
}

TEST(every_byte_value_and_every_length_survives_a_round_trip) {
  Bytes all;
  for (int i = 0; i < 256; i++)
    all.push_back(static_cast<uint8_t>(i));
  for (size_t length = 0; length <= all.size(); length++) {
    const Bytes part(all.begin(), all.begin() + length);
    Bytes out;
    CHECK(decode(encode(part), out));
    CHECK(out == part);
  }
}

TEST(the_characters_that_differ_from_the_url_alphabet_are_the_standard_ones) {
  CHECK_EQ(encode(Bytes{0xfb, 0xff, 0xbf}), "+/+/");
}

TEST(line_breaks_and_spaces_are_skipped) {
  Bytes out;
  CHECK(decode("Zm9v\r\nYmFy\n", out));
  CHECK(out == bytes("foobar"));
  CHECK(decode(" Zm9v YmFy ", out));
  CHECK(out == bytes("foobar"));
}

TEST(anything_else_is_refused_and_leaves_nothing) {
  Bytes out = {1, 2, 3};
  for (const char *bad : {"Zm9v!", "Zm9", "Zg=", "Zg===", "Z", "Zm=v", "=Zm9", "Zm9v=Zm9v", "Zm9\xff", "Zm9v-_-_"}) {
    CHECK(!decode(bad, out));
    CHECK(out.empty());
  }
}

TEST(unused_bits_that_are_not_zero_are_refused) {
  // "Zh==" would be "f" with a nonzero tail; a canonical encoder never writes it.
  Bytes out;
  CHECK(!decode("Zh==", out));
}
