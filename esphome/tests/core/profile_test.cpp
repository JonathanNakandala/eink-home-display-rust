#include <string>
#include <vector>

#include "check.h"
#include "home_display/core/profile.h"

using namespace home_display_profile;

static Profile good() { return Profile{"reTerminal E1003", "0.2.0", 1872, 1404, 16, {"bmp", "png", "qoi"}}; }

TEST(a_profile_the_server_will_keep_is_usable) { CHECK(usable(good())); }

TEST(a_profile_the_server_would_drop_is_not_usable_so_it_is_not_sent) {
  Profile p = good();
  p.model = "";
  CHECK(!usable(p));
  p.model = std::string(33, 'm');
  CHECK(!usable(p));
  p.model = "<script>";
  CHECK(!usable(p));
  p = good();
  p.firmware = "1.0.0+build";
  CHECK(!usable(p));
  p.firmware = "";
  CHECK(!usable(p));
  p = good();
  p.width = 0;
  CHECK(!usable(p));
  p.width = 20001;
  CHECK(!usable(p));
  p = good();
  p.levels = 1;
  CHECK(!usable(p));
  p.levels = 257;
  CHECK(!usable(p));
  p = good();
  p.formats = {"BMP"};
  CHECK(!usable(p));
  p.formats = {""};
  CHECK(!usable(p));
  p.formats = std::vector<std::string>(9, "a");
  CHECK(!usable(p));
  CHECK(encode(p).empty());  // nothing is written for it
}

TEST(the_limits_themselves_are_allowed) {
  Profile p{std::string(32, 'm'), std::string(32, '1'), 20000, 20000, 256, std::vector<std::string>(8, "a")};
  CHECK(usable(p));
  Profile q{"m", "0", 1, 1, 2, {}};
  CHECK(usable(q));
}

TEST(a_profile_is_written_as_the_structure_the_server_reads) {
  // SEQUENCE { 1, "m", 1, 1, 2, SEQUENCE { }, "0" }, byte by byte: the content is 3+3+3+3+3+2+3 = 20 bytes.
  const Bytes expected = {0x30, 0x14,        // SEQUENCE, 20 bytes
                          0x02, 0x01, 0x01,  // version 1
                          0x0C, 0x01, 'm',   // model
                          0x02, 0x01, 0x01,  // width
                          0x02, 0x01, 0x01,  // height
                          0x02, 0x01, 0x02,  // levels
                          0x30, 0x00,        // formats: none
                          0x0C, 0x01, '0'};  // firmware
  CHECK(encode(Profile{"m", "0", 1, 1, 2, {}}) == expected);

  // And with formats: each a UTF8String in a SEQUENCE OF.
  const Bytes with_formats = encode(Profile{"m", "0", 1, 1, 2, {"png", "qoi"}});
  const Bytes formats = {0x30, 0x0A, 0x0C, 0x03, 'p', 'n', 'g', 0x0C, 0x03, 'q', 'o', 'i'};
  CHECK(with_formats.size() == expected.size() + 10);
  CHECK(std::vector<uint8_t>(with_formats.begin() + 17, with_formats.begin() + 29) == formats);
  CHECK_EQ(static_cast<int>(with_formats[1]), 0x14 + 10);

  // A real one is long enough to need the long form of the length at no point, and starts as it should.
  const Bytes real = encode(good());
  CHECK_EQ(static_cast<int>(real[0]), 0x30);
  CHECK_EQ(static_cast<int>(real[1]), static_cast<int>(real.size()) - 2);
}

TEST(numbers_are_written_as_positive_whole_numbers_whatever_their_size) {
  // 127 fits one byte, 128 needs a leading zero so it is not read as negative, 1872 is two bytes, 20000 is two.
  CHECK(integer(0) == (Bytes{0x02, 0x01, 0x00}));
  CHECK(integer(127) == (Bytes{0x02, 0x01, 0x7f}));
  CHECK(integer(128) == (Bytes{0x02, 0x02, 0x00, 0x80}));
  CHECK(integer(1872) == (Bytes{0x02, 0x02, 0x07, 0x50}));
  CHECK(integer(20000) == (Bytes{0x02, 0x02, 0x4e, 0x20}));
  CHECK(integer(0xFFFFFFFFu) == (Bytes{0x02, 0x05, 0x00, 0xff, 0xff, 0xff, 0xff}));
}

TEST(the_formats_a_display_asks_for_are_the_formats_it_says_it_can_decode) {
  CHECK((formats_from_accept("image/bmp, image/png, image/qoi") == std::vector<std::string>{"bmp", "png", "qoi"}));
  // Weights and wildcards: only the plain image types are formats.
  CHECK((formats_from_accept("image/png, image/bmp;q=0.5, */*;q=0.1, text/html") ==
         std::vector<std::string>{"png", "bmp"}));
  CHECK((formats_from_accept("image/*, image/PNG ,image/png") == std::vector<std::string>{"png"}));
  CHECK(formats_from_accept("").empty());
  CHECK(formats_from_accept("text/plain").empty());
  CHECK(formats_from_accept("image/").empty());
}
