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
#include "home_display/core/profile.h"
#include "home_display/core/report.h"
#include "home_display/core/wire.h"

using Bytes = std::vector<uint8_t>;
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

namespace {

// "model|firmware|width|height|levels|formats" as the vectors write a profile.
home_display_profile::Profile profile_from(const std::string &text) {
  const auto parts = vectors::split(text, '|');
  home_display_profile::Profile p;
  p.model = parts.at(0);
  p.firmware = parts.at(1);
  p.width = static_cast<uint32_t>(std::stoul(parts.at(2)));
  p.height = static_cast<uint32_t>(std::stoul(parts.at(3)));
  p.levels = static_cast<uint32_t>(std::stoul(parts.at(4)));
  if (!parts.at(5).empty())
    p.formats = vectors::split(parts.at(5), ',');
  return p;
}

}  // namespace

TEST(the_certificate_request_is_built_byte_for_byte_as_the_vectors_say) {
  int built = 0, with_profile = 0;
  for (const vectors::Case &c : vectors::load("csr.vectors")) {
    // A request only the server need read (a profile it must ignore) is not one the display builds.
    if (c.get("display_builds") != "yes")
      continue;
    built++;
    const std::string challenge = c.get("challenge") == "none" ? "" : c.get("challenge");
    Bytes profile;  // what the display says it is, as DER
    if (c.get("profile") != "none") {
      profile = home_display_profile::encode(profile_from(c.get("profile")));
      with_profile++;
      if (to_hex(profile) != c.get("profile_der"))
        std::printf("      case %s\n", c.name.c_str());
      CHECK_EQ(to_hex(profile), c.get("profile_der"));
    } else {
      CHECK_EQ(c.get("profile_der"), std::string("none"));
    }
    const auto info = home_display_der::request_info(c.get("name"), from_hex(c.get("spki")), challenge, profile);
    if (to_hex(info) != c.get("tbs"))
      std::printf("      case %s\n", c.name.c_str());
    CHECK_EQ(to_hex(info), c.get("tbs"));
    CHECK_EQ(to_hex(home_display_der::request(info, from_hex(c.get("signature")))), c.get("request"));
  }
  CHECK(built >= 5);
  CHECK(with_profile >= 2);  // with the binding and without
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

  const vectors::Case profile = vectors::load("constants.vectors").at(3);
  CHECK_EQ(std::string(home_display_wire::PROFILE_OID), profile.get("oid"));
  CHECK_EQ(to_hex(home_display_der::oid_profile()), profile.get("oid_der"));
  CHECK_EQ(std::to_string(home_display_profile::VERSION), profile.get("version"));
  CHECK_EQ(std::to_string(home_display_profile::limits::MODEL_CHARS), profile.get("model_chars"));
  CHECK_EQ(std::to_string(home_display_profile::limits::FIRMWARE_CHARS), profile.get("firmware_chars"));
  CHECK_EQ(std::to_string(home_display_profile::limits::FORMAT_CHARS), profile.get("format_chars"));
  CHECK_EQ(std::to_string(home_display_profile::limits::MAX_FORMATS), profile.get("max_formats"));
  CHECK_EQ(std::to_string(home_display_profile::limits::MAX_PIXELS), profile.get("max_pixels"));
  CHECK_EQ(std::to_string(home_display_profile::limits::MIN_LEVELS), profile.get("min_levels"));
  CHECK_EQ(std::to_string(home_display_profile::limits::MAX_LEVELS), profile.get("max_levels"));
}
