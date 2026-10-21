#include <set>
#include <utility>
#include <vector>

#include "check.h"
#include "home_display/core/locate.h"

using namespace home_display_locate;
using home_display_service::Server;

static const uint32_t OLD_IP = 192u | (168u << 8) | (0u << 16) | (78u << 24);
static const uint32_t NEW_IP = 192u | (168u << 8) | (0u << 16) | (89u << 24);

// The network, as two functions: which (address, port) pairs accept a connection, and what a search finds (the answer
// for each attempt, in order; an attempt past the end finds nothing). Both count their calls.
struct Net {
  std::set<std::pair<uint32_t, uint16_t>> up;
  std::vector<bool> answers;  // whether attempt n finds the server at NEW_IP
  int reached = 0;
  int searched = 0;
  bool reaches(uint32_t ip, uint16_t port) {
    reached++;
    return up.count({ip, port}) != 0;
  }
  bool once(Server &s) {
    const size_t n = static_cast<size_t>(searched++);
    if (n >= answers.size() || !answers[n])
      return false;
    s = Server();
    s.ip = NEW_IP;
    s.port = 8080;
    s.tls_port = 8443;
    return true;
  }
};

static Where run(Net &net, bool https, uint32_t ip, uint16_t port, uint16_t tls_port, Server &found) {
  return locate(
      https, ip, port, tls_port, [&](uint32_t i, uint16_t p) { return net.reaches(i, p); },
      [&](Server &s) { return net.once(s); }, found);
}

TEST(a_server_that_answers_where_it_was_is_not_searched_for) {
  Net net;
  net.up = {{OLD_IP, 8443}};
  Server found;
  CHECK(run(net, true, OLD_IP, 8080, 8443, found) == Where::REMEMBERED);
  CHECK_EQ(net.searched, 0);
  CHECK_EQ(net.reached, 1);
}

TEST(the_port_checked_is_the_one_the_wake_will_use) {
  CHECK_EQ(port_to_check(true, 8080, 8443), (uint16_t) 8443);
  CHECK_EQ(port_to_check(true, 8080, 0), (uint16_t) 8080);
  CHECK_EQ(port_to_check(false, 8080, 8443), (uint16_t) 8080);
  // Plain HTTP: the HTTP port answers, though nothing is listening on the HTTPS one.
  Net net;
  net.up = {{OLD_IP, 8080}};
  Server found;
  CHECK(run(net, false, OLD_IP, 8080, 8443, found) == Where::REMEMBERED);
  CHECK_EQ(net.searched, 0);
}

TEST(a_server_that_moved_is_found_by_searching_and_not_by_waiting_for_a_request_to_time_out) {
  Net net;
  net.up = {{NEW_IP, 8443}};
  net.answers = {true};
  Server found;
  CHECK(run(net, true, OLD_IP, 8080, 8443, found) == Where::FOUND);
  CHECK_EQ(found.ip, NEW_IP);
  CHECK_EQ(found.tls_port, (uint16_t) 8443);
  CHECK_EQ(net.reached, 1);  // the old address, once
  CHECK_EQ(net.searched, 1);
}

TEST(a_search_that_loses_its_first_query_asks_again) {
  Net net;
  net.answers = {false, false, true};
  Server found;
  CHECK(run(net, true, OLD_IP, 8080, 8443, found) == Where::FOUND);
  CHECK_EQ(net.searched, 3);
}

TEST(a_search_stops_at_the_first_answer) {
  Net net;
  net.answers = {false, true, true};
  Server found;
  CHECK(search([&](Server &s) { return net.once(s); }, found));
  CHECK_EQ(net.searched, 2);
}

TEST(a_search_gives_up_after_its_attempts) {
  Net net;
  Server found;
  CHECK(!search([&](Server &s) { return net.once(s); }, found));
  CHECK_EQ(net.searched, SEARCH_ATTEMPTS);
}

TEST(a_server_nobody_finds_and_nothing_answers_for_is_gone_after_the_old_address_is_tried_twice) {
  Net net;
  Server found;
  CHECK(run(net, true, OLD_IP, 8080, 8443, found) == Where::GONE);
  CHECK_EQ(net.reached, 2);
  CHECK_EQ(net.searched, SEARCH_ATTEMPTS);
}

TEST(an_old_address_that_was_only_slow_is_the_server_when_the_search_finds_nothing) {
  // The first connection ran out of time; by the second the server was there. Multicast is not reliable enough to
  // condemn an address on its silence alone.
  struct Slow : Net {
    bool slow_once = true;
    bool reaches(uint32_t ip, uint16_t port) {
      reached++;
      if (slow_once) {
        slow_once = false;
        return false;
      }
      return ip == OLD_IP && port == 8443;
    }
  } net;
  Server found;
  const Where where = locate(
      true, OLD_IP, (uint16_t) 8080, (uint16_t) 8443, [&](uint32_t i, uint16_t p) { return net.reaches(i, p); },
      [&](Server &s) { return net.once(s); }, found);
  CHECK(where == Where::REMEMBERED);
}

TEST(with_no_address_remembered_it_searches_and_finds_it) {
  Net net;
  net.answers = {true};
  Server found;
  CHECK(run(net, false, 0, 0, 0, found) == Where::FOUND);
  CHECK_EQ(net.reached, 0);
}

TEST(with_no_address_remembered_and_nothing_found_it_goes_on_by_name_and_does_not_give_up) {
  Net net;
  Server found;
  CHECK(run(net, false, 0, 0, 0, found) == Where::UNKNOWN);
  CHECK_EQ(net.reached, 0);
  CHECK_EQ(net.searched, SEARCH_ATTEMPTS);
}

TEST(a_display_that_speaks_tls_and_knows_no_https_port_searches_for_one_though_the_old_address_answers) {
  // The server may have begun to offer HTTPS since the last search.
  Net net;
  net.up = {{OLD_IP, 8080}};
  net.answers = {true};
  Server found;
  CHECK(run(net, true, OLD_IP, 8080, 0, found) == Where::FOUND);
  CHECK_EQ(net.searched, 1);
  CHECK_EQ(net.reached, 0);
}

TEST(a_display_that_cannot_find_an_https_port_goes_on_and_is_not_called_gone) {
  Net net;
  net.up = {{OLD_IP, 8080}};
  Server found;
  CHECK(run(net, true, OLD_IP, 8080, 0, found) == Where::UNKNOWN);
}
