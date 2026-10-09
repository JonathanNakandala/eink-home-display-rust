#include <string>

#include "check.h"
#include "fakes.h"
#include "../eink_join.h"

using namespace fakes;
using eink_join::Joiner;
using eink_join::Outcome;
using eink_pairing::Answer;
using eink_pairing::Standing;
using eink_report::Failure;

static const int64_t START = 1791463200;
static const int64_t DAY = 86400;
static const Bytes ROOT = {0x30, 0x03, 0x02, 0x01, 0x01};
static const Bytes OTHER_ROOT = {0x30, 0x03, 0x02, 0x01, 0x09};
static const Bytes CERT = {0xCE, 0x01};

struct World {
  FakeClock clock;
  FakeIdentity identity;
  FakeEst est;
  FakeVerifier verifier;

  World() {
    est.root_reply = {Fetch::OK, ROOT};
    est.enroll_reply.result = Result::PENDING;
    est.enroll_reply.retry_after_s = 300;
  }
  Outcome run() { return Joiner(clock, identity, est, verifier, "kitchen").run(); }
  // A certificate issued now, as the server does: from this moment, for 90 days.
  void issue(Result result = Result::ISSUED) {
    Reply reply;
    reply.result = result;
    reply.certificate = CERT;
    reply.lifetime = {clock.time, clock.time + 90 * DAY};
    est.enroll_reply = reply;
    est.renew_reply = reply;
  }
  // The display holds a good certificate and the root it came under.
  void pair(int64_t not_before = START, int64_t not_after = START + 90 * DAY) {
    identity.stored = ROOT;
    identity.key = true;
    identity.certificate = true;
    identity.life = {not_before, not_after};
  }
};

TEST(with_the_clock_not_set_nothing_is_asked_of_the_network) {
  World w;
  w.clock.good = false;
  const Outcome out = w.run();
  CHECK(out.standing == Standing::CLOCK_NOT_SET);
  CHECK(out.failure == Failure::CLOCK);
  CHECK(!out.paired);
  CHECK_EQ(w.est.requests(), 0);
}

TEST(a_new_display_fetches_the_root_makes_its_key_and_asks_then_waits_showing_the_code) {
  World w;
  const Outcome out = w.run();
  CHECK_EQ(w.est.roots_fetched, 1);
  CHECK_EQ(w.identity.keys_made, 1);
  CHECK_EQ(w.est.enrolled, 1);
  CHECK(out.standing == Standing::WAITING);
  CHECK(out.failure == Failure::APPROVAL);
  CHECK_EQ(out.retry_after_s, 300u);
  CHECK(!out.paired);
  CHECK(w.identity.last == Answer::PENDING);
  // The code is the one worked out from the root it saw and its own key.
  CHECK_EQ(out.code, eink_pairing::code(ROOT, "kitchen", w.identity.key_spki));
  CHECK_EQ(w.est.enrolled_against, ROOT);
}

TEST(a_root_that_no_certificate_has_confirmed_is_not_kept) {
  World w;
  w.run();
  CHECK(w.identity.stored.empty());
  CHECK_EQ(w.identity.roots_saved, 0);
}

TEST(each_wake_while_waiting_fetches_the_root_again_so_one_bad_fetch_cannot_wedge_it) {
  World w;
  w.run();
  w.run();
  w.run();
  CHECK_EQ(w.est.roots_fetched, 3);
  CHECK_EQ(w.est.enrolled, 3);
  CHECK_EQ(w.identity.keys_made, 1);  // the key is made once
}

TEST(someone_in_the_middle_who_shows_another_root_makes_the_codes_differ) {
  World honest, middle;
  middle.est.root_reply = {Fetch::OK, OTHER_ROOT};
  const std::string on_the_server = honest.run().code;
  const std::string on_the_panel = middle.run().code;
  CHECK(on_the_server != on_the_panel);
}

TEST(once_approved_the_certificate_is_kept_with_the_root_it_came_under) {
  World w;
  w.run();
  w.issue();
  const Outcome out = w.run();
  CHECK(out.paired);
  CHECK(out.standing == Standing::PAIRED);
  CHECK(out.failure == Failure::NONE);
  CHECK_EQ(w.identity.roots_saved, 1);
  CHECK(w.identity.stored == ROOT);
  CHECK_EQ(w.identity.certificates_saved, 1);
  CHECK(w.identity.last == Answer::NONE);
  CHECK(w.verifier.checked_against == ROOT);
}

TEST(the_certificate_is_checked_through_the_intermediates_the_server_presented) {
  World w;
  w.issue();
  w.est.enroll_reply.intermediates = {Bytes{0x11}, Bytes{0x22}};
  CHECK(w.run().paired);
  CHECK_EQ(w.verifier.checked_through.size(), (size_t) 2);
  CHECK(w.verifier.checked_through[0] == Bytes{0x11});
}

TEST(a_certificate_not_signed_under_the_root_is_not_kept) {
  World w;
  w.issue();
  w.verifier.accepts = false;
  const Outcome out = w.run();
  CHECK(!out.paired);
  CHECK(out.failure == Failure::CERTIFICATE);
  CHECK_EQ(w.identity.certificates_saved, 0);
  CHECK_EQ(w.identity.roots_saved, 0);
}

TEST(a_paired_display_goes_on_without_touching_the_network) {
  World w;
  w.pair();
  w.clock.time = START + 10 * DAY;
  const Outcome out = w.run();
  CHECK(out.paired && out.standing == Standing::PAIRED);
  CHECK_EQ(w.est.requests(), 0);
}

TEST(with_a_third_of_the_certificate_left_it_renews_with_the_same_key) {
  World w;
  w.pair();
  w.clock.time = START + 61 * DAY;
  w.issue();
  const Outcome out = w.run();
  CHECK_EQ(w.est.renewed, 1);
  CHECK_EQ(w.est.enrolled, 0);
  CHECK_EQ(w.est.roots_fetched, 0);
  CHECK_EQ(w.identity.keys_made, 0);
  CHECK(out.paired);
  CHECK_EQ(w.identity.certificates_saved, 1);
  CHECK_EQ(w.identity.life.not_after, START + 151 * DAY);
}

TEST(a_certificate_that_ended_is_asked_for_again_with_the_same_key_and_no_owner) {
  World w;
  w.pair();
  w.clock.time = START + 200 * DAY;
  w.issue();
  const Outcome out = w.run();
  CHECK_EQ(w.est.enrolled, 1);
  CHECK_EQ(w.est.roots_fetched, 0);
  CHECK_EQ(w.identity.keys_made, 0);
  CHECK(out.paired);
}

TEST(an_expired_display_the_server_does_not_know_waits_for_the_owner_and_loses_nothing) {
  World w;
  w.pair();
  w.est.enroll_reply.result = Result::REFUSED;
  w.clock.time = START + 200 * DAY;
  const Outcome out = w.run();
  CHECK(out.standing == Standing::NOT_RECOGNISED);
  CHECK(out.failure == Failure::UNRECOGNISED);
  CHECK(!out.paired);
  CHECK_EQ(out.code, eink_pairing::code(ROOT, "kitchen", w.identity.key_spki));
  // Pairing is removed only by the owner, never by an error.
  CHECK(w.identity.key && w.identity.certificate && w.identity.stored == ROOT);
  CHECK(w.identity.last == Answer::REFUSED);
}

TEST(a_renewal_the_server_refuses_is_asked_again_as_a_new_display_in_the_same_wake) {
  World w;
  w.pair();
  w.est.renew_reply.result = Result::REFUSED;
  w.est.enroll_reply.result = Result::PENDING;
  w.clock.time = START + 70 * DAY;
  const Outcome out = w.run();
  CHECK_EQ(w.est.renewed, 1);
  CHECK_EQ(w.est.enrolled, 1);
  CHECK(out.standing == Standing::WAITING);
  CHECK(w.identity.key && w.identity.certificate);
}

TEST(a_server_that_cannot_be_reached_is_reported_as_the_server_and_nothing_changes) {
  World w;
  w.est.root_reply.result = Fetch::UNREACHABLE;
  Outcome out = w.run();
  CHECK(out.failure == Failure::SERVER);
  CHECK_EQ(w.identity.keys_made, 0);

  World v;
  v.est.enroll_reply.result = Result::UNREACHABLE;
  out = v.run();
  CHECK(out.failure == Failure::SERVER);
  CHECK(v.identity.last == Answer::NONE);  // it never answered: nothing to remember
}

TEST(a_root_reply_with_no_usable_root_is_not_believed) {
  World w;
  w.est.root_reply = {Fetch::BAD, {}};
  const Outcome out = w.run();
  CHECK(out.failure == Failure::SERVER);
  CHECK_EQ(w.est.enrolled, 0);
}

TEST(a_refused_certificate_on_the_connection_is_reported_as_one) {
  World w;
  w.pair();
  w.est.renew_reply.result = Result::TLS_REFUSED;
  w.clock.time = START + 70 * DAY;
  const Outcome out = w.run();
  CHECK(out.failure == Failure::CERTIFICATE);
  CHECK(!out.paired);
  CHECK(w.identity.certificate);  // kept, whatever the server thought of it
}

TEST(a_root_built_into_the_firmware_is_never_fetched_and_always_the_one_trusted) {
  World w;
  w.identity.compiled = OTHER_ROOT;
  const Outcome out = w.run();
  CHECK_EQ(w.est.roots_fetched, 0);
  CHECK(w.est.enrolled_against == OTHER_ROOT);
  CHECK_EQ(out.code, eink_pairing::code(OTHER_ROOT, "kitchen", w.identity.key_spki));
  // A stored root does not beat it.
  World x;
  x.identity.compiled = OTHER_ROOT;
  x.identity.stored = ROOT;
  x.run();
  CHECK(x.est.enrolled_against == OTHER_ROOT);
}

TEST(a_root_built_in_is_not_saved_when_a_certificate_comes) {
  World w;
  w.identity.compiled = ROOT;
  w.issue();
  CHECK(w.run().paired);
  CHECK_EQ(w.identity.roots_saved, 0);
}

TEST(what_is_kept_but_cannot_be_read_is_left_alone_and_nothing_is_tried) {
  World w;
  w.identity.readable_ = false;
  const Outcome out = w.run();
  CHECK(out.failure == Failure::MEMORY);
  CHECK(!out.paired);
  CHECK_EQ(w.est.requests(), 0);
  CHECK_EQ(w.identity.keys_made, 0);
  CHECK_EQ(w.identity.roots_saved + w.identity.certificates_saved, 0);
}

TEST(a_flash_that_cannot_be_written_is_reported_and_is_not_taken_for_success) {
  World w;
  w.identity.can_make_key = false;
  CHECK(w.run().failure == Failure::MEMORY);

  World v;
  v.issue();
  v.identity.can_save_certificate = false;
  Outcome out = v.run();
  CHECK(out.failure == Failure::MEMORY);
  CHECK(!out.paired);
  CHECK_EQ(v.identity.certificates_saved, 0);

  World u;
  u.issue();
  u.identity.can_save_root = false;
  out = u.run();
  CHECK(out.failure == Failure::MEMORY);
  CHECK(!u.identity.certificate);  // no certificate is kept without the root that checks it
}

TEST(a_certificate_from_the_future_means_the_clock_is_behind_and_nothing_is_asked) {
  World w;
  w.pair();
  w.clock.time = START - DAY;
  const Outcome out = w.run();
  CHECK(out.standing == Standing::CLOCK_NOT_SET);
  CHECK_EQ(w.est.requests(), 0);
}

TEST(a_certificate_that_is_already_over_or_not_yet_begun_is_not_asked_for_again) {
  World w;
  w.issue();
  w.est.enroll_reply.lifetime = {START - 100 * DAY, START - DAY};  // over already
  Outcome out = w.run();
  CHECK(out.failure == Failure::CERTIFICATE);
  CHECK(!out.paired);
  CHECK_EQ(w.est.enrolled, 1);  // once, not again and again

  World v;
  v.issue();
  v.est.enroll_reply.lifetime = {START + DAY, START + 90 * DAY};  // starts tomorrow by this clock
  out = v.run();
  CHECK(out.failure == Failure::CLOCK);
  CHECK(out.standing == Standing::CLOCK_NOT_SET);
  CHECK_EQ(v.est.enrolled, 1);
}
