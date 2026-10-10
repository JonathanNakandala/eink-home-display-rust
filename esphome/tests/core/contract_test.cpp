// The display's half of the contract with the server: testdata/contract/ holds what both must agree on, and the
// server's tests (src/contract_tests.rs) read the same files. Here, what the display builds is what the vectors say,
// and what it takes from the server is read as they say.
#include <string>
#include <vector>

#include "check.h"
#include "vectors.h"
#include "home_display/core/base64.h"
#include "home_display/core/battery.h"
#include "home_display/core/der.h"
#include "home_display/core/pairing.h"
#include "home_display/core/report.h"
#include "home_display/core/wire.h"

using vectors::from_hex;
using vectors::to_hex;

TEST(the_pairing_code_is_what_the_vectors_say_for_every_case) {
  for (const vectors::Case &c : vectors::load("pairing_code.vectors")) {
    const std::string code =
        home_display_pairing::code(from_hex(c.get("root")), c.get("name"), from_hex(c.get("spki")));
    if (code != c.get("code"))
      std::printf("      case %s\n", c.name.c_str());
    CHECK_EQ(code, c.get("code"));
  }
}

TEST(base64_is_what_the_vectors_say_both_ways) {
  for (const vectors::Case &c : vectors::load("base64.vectors")) {
    const auto bytes = from_hex(c.get("bytes"));
    if (home_display_base64::encode(bytes) != c.get("text"))
      std::printf("      case %s\n", c.name.c_str());
    CHECK_EQ(home_display_base64::encode(bytes), c.get("text"));
    home_display_base64::Bytes decoded;
    CHECK(home_display_base64::decode(c.get("text"), decoded));
    CHECK_EQ(to_hex(decoded), c.get("bytes"));
  }
}

TEST(the_certificate_request_is_built_byte_for_byte_as_the_vectors_say) {
  for (const vectors::Case &c : vectors::load("csr.vectors")) {
    const std::string challenge = c.get("challenge") == "none" ? "" : c.get("challenge");
    const auto info = home_display_der::request_info(c.get("name"), from_hex(c.get("spki")), challenge);
    if (to_hex(info) != c.get("tbs"))
      std::printf("      case %s\n", c.name.c_str());
    CHECK_EQ(to_hex(info), c.get("tbs"));
    CHECK_EQ(to_hex(home_display_der::request(info, from_hex(c.get("signature")))), c.get("request"));
  }
}

namespace {

home_display_report::Failure failure_called(const std::string &name) {
  using home_display_report::Failure;
  for (int v = 1; v <= static_cast<int>(home_display_report::LAST_FAILURE); v++)
    if (name == home_display_report::failure_label(static_cast<Failure>(v)))
      return static_cast<Failure>(v);
  CHECK(false);  // not a name the display has
  return Failure::NONE;
}

}  // namespace

TEST(the_check_in_is_written_exactly_as_the_vectors_say) {
  using namespace home_display_report;
  for (const vectors::Case &c : vectors::load("report.vectors")) {
    Battery battery = {false, 0, 0, "ok"};
    std::string state;  // the state's text must outlive the query
    const std::string reading = c.get("battery", "none");
    if (reading != "none") {
      const auto parts = vectors::split(reading, ' ');
      state = parts.at(2);
      battery = {true, static_cast<uint32_t>(std::stoul(parts.at(0))), std::stoi(parts.at(1)), state.c_str()};
    }
    Last last = EMPTY;
    if (c.get("failure", "none") != "none")
      set_failure(last, failure_called(c.get("failure")));
    if (c.get("wake_ms", "none") != "none")
      set_wake(last, static_cast<uint32_t>(std::stoull(c.get("wake_ms"))));
    set_connection(last, static_cast<uint32_t>(std::stoull(c.get("tls_ms", "0"))),
                   static_cast<uint32_t>(std::stoull(c.get("heap_min", "0"))));
    const std::string firmware = c.get("firmware", "none") == "none" ? "" : c.get("firmware");
    const std::string query =
        home_display_report::query(c.get("device"), static_cast<unsigned>(std::stoul(c.get("failed_wakes"))), battery,
                                   std::stoi(c.get("rssi", "0")), last, firmware);
    if (query != c.get("query"))
      std::printf("      case %s\n", c.name.c_str());
    CHECK_EQ(query, c.get("query"));
  }
}

TEST(the_names_and_numbers_both_sides_share_are_what_the_vectors_say) {
  const vectors::Case wire = vectors::load("constants.vectors").at(0);
  CHECK_EQ(std::string(home_display_wire::SERVER_NAME), wire.get("server_name"));
  CHECK_EQ(std::string(home_display_wire::BINDING_LABEL), wire.get("binding_label"));
  CHECK_EQ(std::to_string(home_display_wire::BINDING_LENGTH), wire.get("binding_length"));

  const vectors::Case code = vectors::load("constants.vectors").at(1);
  CHECK_EQ(std::string(home_display_pairing::ALPHABET), code.get("alphabet"));
  CHECK_EQ(std::to_string(home_display_pairing::CODE_CHARACTERS), code.get("characters"));
  CHECK_EQ(std::to_string(home_display_pairing::CODE_GROUP), code.get("group"));
  CHECK_EQ(std::string(home_display_pairing::HASH_LABEL), code.get("hash_label"));

  const vectors::Case report = vectors::load("constants.vectors").at(2);
  // Every failure the display can name, in the order of the enum, is a name the server knows, and no other.
  std::vector<std::string> names;
  for (int v = 1; v <= static_cast<int>(home_display_report::LAST_FAILURE); v++)
    names.push_back(home_display_report::failure_label(static_cast<home_display_report::Failure>(v)));
  CHECK(names == vectors::split(report.get("failure_names"), ','));
  std::vector<std::string> states;
  for (auto s : {home_display_battery::State::OK, home_display_battery::State::LOW, home_display_battery::State::EMPTY})
    states.push_back(home_display_battery::state_name(s));
  CHECK(states == vectors::split(report.get("battery_states"), ','));
  CHECK_EQ(std::to_string(home_display_report::MAX_FIRMWARE_CHARS), report.get("firmware_version_longest"));
}
