#include <string>
#include <vector>

#include "check.h"
#include "home_display/core/pairing.h"

using namespace home_display_pairing;
using home_display_report::Failure;
using home_display_trust::RootSource;

// The example in README.md and in src/domain/models/pairing.rs: the same inputs, the same code.
static std::vector<uint8_t> authority_der() { return {0x30, 0x03, 0x02, 0x01, 0x01}; }

static std::vector<uint8_t> example_key() {
  std::vector<uint8_t> key;
  for (int i = 0; i < 91; i++)
    key.push_back(static_cast<uint8_t>(i));
  return key;
}

TEST(the_example_in_the_documentation_and_in_the_server_gives_the_same_code) {
  CHECK_EQ(home_display_sha256::hex(home_display_sha256::of(authority_der())),
           "1b65f68a522c858715f5dd951cd0402dc16691778814bf0759822b7a257421d0");
  CHECK_EQ(code(authority_der(), "reterminal-e1003-a1b2c3", example_key()), "JGWP-14YW-3BT0");
}

TEST(a_code_is_twelve_characters_in_three_groups_of_the_alphabet) {
  for (uint8_t seed = 0; seed < 50; seed++) {
    const std::string c = code({seed}, "kitchen", {seed, 2, 3});
    CHECK_EQ(c.size(), (size_t) 14);
    CHECK_EQ(c[4], '-');
    CHECK_EQ(c[9], '-');
    for (size_t i = 0; i < c.size(); i++) {
      if (i == 4 || i == 9)
        continue;
      CHECK(std::string(ALPHABET).find(c[i]) != std::string::npos);
    }
  }
}

TEST(the_code_changes_with_the_root_the_name_and_the_key) {
  const std::string base = code({1}, "kitchen", {2});
  CHECK(code({9}, "kitchen", {2}) != base);
  CHECK(code({1}, "hall", {2}) != base);
  CHECK(code({1}, "kitchen", {9}) != base);
  CHECK_EQ(code({1}, "kitchen", {2}), base);
}

TEST(moving_a_byte_between_the_name_and_the_key_gives_another_code) {
  // The parts are delimited by their lengths, so the same bytes split differently are different input.
  CHECK(code({1}, "ab", {'c', 'd'}) != code({1}, "abc", {'d'}));
}

TEST(the_alphabet_has_no_look_alikes) {
  const std::string alphabet = ALPHABET;
  CHECK_EQ(alphabet.size(), (size_t) 32);
  for (char c : {'I', 'L', 'O', 'U'})
    CHECK(alphabet.find(c) == std::string::npos);
}

// A display that holds a root and a key, and (optionally) a certificate from START to END.
static const int64_t START = 1791463200;
static const int64_t DAY = 86400;
static const int64_t END = START + 90 * DAY;

static Stored joined(RootSource root = RootSource::STORED) { return Stored{root, true, true, START, END}; }
static Stored with_certificate_only_for(RootSource root, bool key, bool certificate) {
  return Stored{root, key, certificate, START, END};
}

TEST(nothing_is_tried_until_the_clock_is_usable) {
  for (Answer answer : {Answer::NONE, Answer::PENDING, Answer::REFUSED}) {
    const Next n = next(joined(), START + DAY, false, answer);
    CHECK(n.standing == Standing::CLOCK_NOT_SET);
    CHECK(n.action == Step::NOTHING);
  }
  // Not even fetching the root: that is a TLS connection too.
  const Next fresh = next(with_certificate_only_for(RootSource::NONE, false, false), START, false, Answer::NONE);
  CHECK(fresh.action == Step::NOTHING);
}

TEST(a_display_with_nothing_fetches_the_root_then_makes_a_key_then_asks) {
  Next n = next(with_certificate_only_for(RootSource::NONE, false, false), START, true, Answer::NONE);
  CHECK(n.standing == Standing::NO_ROOT && n.action == Step::FETCH_ROOT);
  n = next(with_certificate_only_for(RootSource::STORED, false, false), START, true, Answer::NONE);
  CHECK(n.standing == Standing::ASKING && n.action == Step::MAKE_KEY);
  n = next(with_certificate_only_for(RootSource::STORED, true, false), START, true, Answer::NONE);
  CHECK(n.standing == Standing::ASKING && n.action == Step::ENROLL);
}

TEST(a_compiled_in_root_skips_fetching_it) {
  const Next n = next(with_certificate_only_for(RootSource::COMPILED, false, false), START, true, Answer::NONE);
  CHECK(n.action == Step::MAKE_KEY);
}

TEST(while_the_owner_has_not_approved_it_keeps_asking_and_shows_the_code) {
  const Next n = next(with_certificate_only_for(RootSource::STORED, true, false), START, true, Answer::PENDING);
  CHECK(n.standing == Standing::WAITING && n.action == Step::ENROLL);
}

TEST(a_refusal_to_a_display_with_no_certificate_is_not_being_recognised_and_it_keeps_asking) {
  const Next n = next(with_certificate_only_for(RootSource::STORED, true, false), START, true, Answer::REFUSED);
  CHECK(n.standing == Standing::NOT_RECOGNISED && n.action == Step::ENROLL);
}

TEST(a_paired_display_carries_on_until_a_third_of_the_certificate_is_left) {
  Next n = next(joined(), START + DAY, true, Answer::NONE);
  CHECK(n.standing == Standing::PAIRED && n.action == Step::USE);
  n = next(joined(), START + 59 * DAY, true, Answer::NONE);
  CHECK(n.standing == Standing::PAIRED);
  n = next(joined(), START + 60 * DAY, true, Answer::NONE);
  CHECK(n.standing == Standing::RENEWING && n.action == Step::RENEW);
}

TEST(a_certificate_that_ended_is_asked_for_again_with_the_same_key_and_no_owner) {
  const Next n = next(joined(), END + 30 * DAY, true, Answer::NONE);
  CHECK(n.standing == Standing::EXPIRED && n.action == Step::ENROLL);
}

TEST(an_expired_display_the_server_does_not_know_waits_for_the_owner_like_a_new_one) {
  Next n = next(joined(), END + DAY, true, Answer::REFUSED);
  CHECK(n.standing == Standing::NOT_RECOGNISED && n.action == Step::ENROLL);
  n = next(joined(), END + DAY, true, Answer::PENDING);
  CHECK(n.standing == Standing::WAITING && n.action == Step::ENROLL);
}

TEST(a_renewal_the_server_refuses_means_it_no_longer_knows_the_display) {
  const Next n = next(joined(), START + 70 * DAY, true, Answer::REFUSED);
  CHECK(n.standing == Standing::NOT_RECOGNISED && n.action == Step::ENROLL);
}

TEST(a_display_refused_while_paired_asks_again_and_keeps_what_it_holds) {
  // The standing and action are all that is decided; nothing here deletes the key, certificate or root.
  const Next n = next(joined(), START + DAY, true, Answer::REFUSED);
  CHECK(n.standing == Standing::NOT_RECOGNISED && n.action == Step::ENROLL);
}

TEST(a_certificate_from_the_future_means_the_clock_is_behind) {
  const Next n = next(joined(), START - DAY, true, Answer::NONE);
  CHECK(n.standing == Standing::CLOCK_NOT_SET && n.action == Step::NOTHING);
}

TEST(a_standing_is_reported_to_the_server_as_the_failure_it_is) {
  CHECK(failure_for(Standing::CLOCK_NOT_SET) == Failure::CLOCK);
  CHECK(failure_for(Standing::WAITING) == Failure::APPROVAL);
  CHECK(failure_for(Standing::NOT_RECOGNISED) == Failure::UNRECOGNISED);
  for (Standing fine : {Standing::PAIRED, Standing::RENEWING, Standing::EXPIRED, Standing::ASKING, Standing::NO_ROOT})
    CHECK(failure_for(fine) == Failure::NONE);
}

// The font on the panel has only these glyphs (packages/display.yaml); a character outside them would draw as nothing.
static const std::string GLYPHS = " :@%-.0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

TEST(the_panel_shows_the_code_only_where_the_owner_has_to_type_it_and_uses_only_glyphs_the_font_has) {
  const std::string c = "JGWP-14YW-3BT0";
  CHECK_EQ(notice(Standing::WAITING, c), "Waiting for approval: JGWP-14YW-3BT0");
  CHECK_EQ(notice(Standing::NOT_RECOGNISED, c), "Not recognised - ask the owner to approve: JGWP-14YW-3BT0");
  CHECK_EQ(notice(Standing::PAIRED, c), "");
  CHECK_EQ(notice(Standing::RENEWING, c), "");
  for (uint8_t value = 0; value <= static_cast<uint8_t>(Standing::CLOCK_NOT_SET); value++)
    CHECK(notice(static_cast<Standing>(value), c).find_first_not_of(GLYPHS) == std::string::npos);
}
