#include "check.h"
#include "../eink_format.h"

using namespace eink_format;

TEST(the_ip_is_read_first_octet_in_the_lowest_byte) {
  // 192.168.1.20, as lwIP stores it.
  const uint32_t ip = 192u | (168u << 8) | (1u << 16) | (20u << 24);
  CHECK_EQ(url(ip, 8080, ""), "http://192.168.1.20:8080");
  CHECK_EQ(url(ip, 8080, "/image"), "http://192.168.1.20:8080/image");
}

TEST(the_edges_of_an_address_and_port) {
  CHECK_EQ(url(0, 0, ""), "http://0.0.0.0:0");
  CHECK_EQ(url(0xFFFFFFFFu, 65535, "/plan"), "http://255.255.255.255:65535/plan");
}

TEST(an_age_is_in_the_largest_sensible_unit) {
  CHECK_EQ(age(0), "under 1 min");
  CHECK_EQ(age(59), "under 1 min");
  CHECK_EQ(age(60), "1 min");
  CHECK_EQ(age(59 * 60 + 59), "59 min");
  CHECK_EQ(age(3600), "1 h 0 min");
  CHECK_EQ(age(3 * 3600 + 20 * 60), "3 h 20 min");
  CHECK_EQ(age(26 * 3600), "26 h 0 min");
}
