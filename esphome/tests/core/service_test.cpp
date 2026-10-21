#include <string>

#include "check.h"
#include "home_display/core/service.h"

using namespace home_display_service;

// 192.168.1.20, as lwIP stores it.
static const uint32_t IP = 192u | (168u << 8) | (1u << 16) | (20u << 24);

// What the server announces for each of its `transport` settings (advertise.rs).
static Announcement http_only() {
  Announcement a;
  a.instance = "Home display";
  a.port = 8080;
  a.ip = IP;
  a.txtvers = "1";
  a.path = "/image";
  a.format = "bmp";
  return a;
}

static Announcement prefer_https() {
  Announcement a = http_only();
  a.tlsport = "8443";
  a.secure = "optional";
  return a;
}

static Announcement https_only() {
  Announcement a = http_only();
  a.port = 8443;
  a.tlsport = "8443";
  a.secure = "required";
  return a;
}

TEST(the_transports_are_read_by_name) {
  static_assert(transport_from("http") == Transport::HTTP);
  static_assert(transport_from("prefer-https") == Transport::PREFER_HTTPS);
  static_assert(transport_from("https") == Transport::HTTPS);
  CHECK(transport_from("https") == Transport::HTTPS);
}

TEST(only_an_https_display_looks_under_https) {
  CHECK_EQ(std::string(service_type(Transport::HTTP)), "_http");
  CHECK_EQ(std::string(service_type(Transport::PREFER_HTTPS)), "_http");
  CHECK_EQ(std::string(service_type(Transport::HTTPS)), "_https");
}

TEST(a_plain_http_server_is_found_as_before) {
  Server server;
  CHECK(accept(http_only(), "", Transport::HTTP, server));
  CHECK_EQ(server.ip, IP);
  CHECK_EQ(server.port, (uint16_t) 8080);
  CHECK_EQ(server.tls_port, (uint16_t) 0);
  CHECK(!server.secure_required);
  CHECK_EQ(server.format, "bmp");
}

TEST(the_https_port_is_remembered_beside_the_http_one_when_the_server_offers_it) {
  Server server;
  CHECK(accept(prefer_https(), "", Transport::PREFER_HTTPS, server));
  CHECK_EQ(server.port, (uint16_t) 8080);
  CHECK_EQ(server.tls_port, (uint16_t) 8443);
  CHECK(!server.secure_required);
  // A display set to plain HTTP still learns it, and ignores it.
  CHECK(accept(prefer_https(), "", Transport::HTTP, server));
  CHECK_EQ(server.tls_port, (uint16_t) 8443);
}

TEST(an_https_only_server_has_its_one_port_taken_as_https) {
  Server server;
  CHECK(accept(https_only(), "", Transport::HTTPS, server));
  CHECK_EQ(server.port, (uint16_t) 0);  // no plain HTTP to speak
  CHECK_EQ(server.tls_port, (uint16_t) 8443);
  CHECK(server.secure_required);
}

TEST(an_https_announcement_whose_ports_disagree_is_not_trusted) {
  Announcement a = https_only();
  a.tlsport = "9443";
  Server server;
  CHECK(!accept(a, "", Transport::HTTPS, server));
}

TEST(an_https_announcement_without_tlsport_still_means_its_port) {
  Announcement a = https_only();
  a.tlsport = "";
  Server server;
  CHECK(accept(a, "", Transport::HTTPS, server));
  CHECK_EQ(server.tls_port, (uint16_t) 8443);
}

TEST(a_service_that_is_not_ours_is_skipped) {
  Server server;
  Announcement other = http_only();
  other.txtvers = "";
  CHECK(!accept(other, "", Transport::HTTP, server));
  other = http_only();
  other.txtvers = "2";
  CHECK(!accept(other, "", Transport::HTTP, server));
  other = http_only();
  other.path = "";
  CHECK(!accept(other, "", Transport::HTTP, server));
}

TEST(an_answer_with_no_address_or_port_cannot_be_used) {
  Server server;
  Announcement a = http_only();
  a.ip = 0;
  CHECK(!accept(a, "", Transport::HTTP, server));
  a = http_only();
  a.port = 0;
  CHECK(!accept(a, "", Transport::HTTP, server));
}

TEST(a_named_server_is_picked_from_several) {
  Server server;
  Announcement a = http_only();
  CHECK(accept(a, "Home display", Transport::HTTP, server));
  CHECK(!accept(a, "Kitchen", Transport::HTTP, server));
  CHECK(accept(a, "", Transport::HTTP, server));  // empty takes the first
}

TEST(a_port_is_a_number_from_1_to_65535) {
  CHECK_EQ(port_from("8443"), (uint16_t) 8443);
  CHECK_EQ(port_from("1"), (uint16_t) 1);
  CHECK_EQ(port_from("65535"), (uint16_t) 65535);
  for (const char *bad : {"", "0", "65536", "99999", "100000", "-1", "+1", "80a", " 80", "8 0", "eight"})
    CHECK_EQ(port_from(bad), (uint16_t) 0);
}

TEST(a_failed_accept_leaves_what_the_caller_had) {
  Server server;
  CHECK(accept(http_only(), "", Transport::HTTP, server));
  Announcement other = http_only();
  other.txtvers = "";
  CHECK(!accept(other, "", Transport::HTTP, server));
  CHECK_EQ(server.port, (uint16_t) 8080);
}

static uint32_t ip4(uint32_t a, uint32_t b, uint32_t c, uint32_t d) { return a | (b << 8) | (c << 16) | (d << 24); }

TEST(of_several_addresses_the_one_on_the_displays_own_network_is_taken) {
  const uint32_t mask = ip4(255, 255, 254, 0);  // a /23, as the display's network is
  const uint32_t own = ip4(192, 168, 0, 74);
  // Order as a machine with a container bridge and a VPN might announce them.
  const std::vector<uint32_t> all = {ip4(172, 17, 0, 1), ip4(10, 5, 0, 2), ip4(192, 168, 1, 40)};
  CHECK_EQ(choose_ip(all, own, mask), ip4(192, 168, 1, 40));  // 192.168.1.x is inside the /23
}

TEST(with_no_address_on_its_network_or_no_network_of_its_own_the_first_address_is_taken) {
  const std::vector<uint32_t> all = {ip4(172, 17, 0, 1), ip4(10, 5, 0, 2)};
  CHECK_EQ(choose_ip(all, ip4(192, 168, 0, 74), ip4(255, 255, 255, 0)), ip4(172, 17, 0, 1));
  CHECK_EQ(choose_ip(all, 0, 0), ip4(172, 17, 0, 1));
}

TEST(no_addresses_is_no_address_and_a_zero_one_is_skipped) {
  CHECK_EQ(choose_ip({}, ip4(192, 168, 0, 74), ip4(255, 255, 255, 0)), (uint32_t) 0);
  CHECK_EQ(choose_ip({0, ip4(10, 0, 0, 5)}, 0, 0), ip4(10, 0, 0, 5));
}
