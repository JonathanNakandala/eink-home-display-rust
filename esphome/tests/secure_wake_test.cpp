#include "check.h"
#include "../eink_secure_wake.h"

using namespace eink_secure_wake;
using eink_join::Outcome;
using eink_pairing::Standing;
using eink_report::Failure;
using eink_service::Transport;
using Start = eink_link::Start;

static Outcome outcome(Standing standing, Failure failure, bool paired = false) {
  Outcome o;
  o.standing = standing;
  o.failure = failure;
  o.paired = paired;
  return o;
}

static const Outcome PAIRED = outcome(Standing::PAIRED, Failure::NONE, true);
static const Outcome WAITING = outcome(Standing::WAITING, Failure::APPROVAL);
static const Outcome UNKNOWN = outcome(Standing::NOT_RECOGNISED, Failure::UNRECOGNISED);
static const Outcome NO_CLOCK = outcome(Standing::CLOCK_NOT_SET, Failure::CLOCK);
static const Outcome UNREACHABLE = outcome(Standing::ASKING, Failure::SERVER);
static const Outcome REFUSED = outcome(Standing::PAIRED, Failure::CERTIFICATE);

TEST(plain_http_never_joins_or_changes_anything) {
  for (const Outcome &o : {PAIRED, WAITING, NO_CLOCK}) {
    const Verdict v = decide(Transport::HTTP, true, o);
    CHECK(v.route == Route::PLAIN && v.failure == Failure::NONE && !v.prompt);
  }
}

TEST(a_paired_display_speaks_tls_in_both_secure_modes) {
  for (Transport t : {Transport::PREFER_HTTPS, Transport::HTTPS}) {
    const Verdict v = decide(t, true, PAIRED);
    CHECK(v.route == Route::SECURE && v.failure == Failure::NONE && !v.prompt);
  }
}

TEST(a_display_that_waits_for_the_owner_is_prompted_in_both_and_waits_only_in_https) {
  for (const Outcome &o : {WAITING, UNKNOWN}) {
    const Verdict prefer = decide(Transport::PREFER_HTTPS, true, o);
    CHECK(prefer.route == Route::PLAIN && prefer.prompt && prefer.failure == o.failure);
    const Verdict only = decide(Transport::HTTPS, true, o);
    CHECK(only.route == Route::WAIT_FOR_OWNER && only.prompt && only.failure == o.failure);
  }
}

TEST(any_other_failure_falls_back_to_plain_when_preferred_and_fails_the_wake_when_not) {
  for (const Outcome &o : {NO_CLOCK, UNREACHABLE, REFUSED}) {
    const Verdict prefer = decide(Transport::PREFER_HTTPS, true, o);
    CHECK(prefer.route == Route::PLAIN && !prefer.prompt && prefer.failure == o.failure);
    const Verdict only = decide(Transport::HTTPS, true, o);
    CHECK(only.route == Route::FAIL && !only.prompt && only.failure == o.failure);
  }
}

TEST(a_server_with_no_https_port_cannot_be_joined_and_is_reported_as_the_server) {
  for (const Outcome &o : {PAIRED, WAITING}) {  // whatever join said, it is ignored
    const Verdict prefer = decide(Transport::PREFER_HTTPS, false, o);
    CHECK(prefer.route == Route::PLAIN && !prefer.prompt && prefer.failure == Failure::SERVER);
    const Verdict only = decide(Transport::HTTPS, false, o);
    CHECK(only.route == Route::FAIL && !only.prompt && only.failure == Failure::SERVER);
  }
}

TEST(https_only_never_ends_up_on_plain_http) {
  for (bool addressable : {true, false})
    for (const Outcome &o : {PAIRED, WAITING, UNKNOWN, NO_CLOCK, UNREACHABLE, REFUSED})
      CHECK(decide(Transport::HTTPS, addressable, o).route != Route::PLAIN);
}

TEST(only_waiting_and_not_recognised_need_the_owner) {
  CHECK(needs_owner(Standing::WAITING));
  CHECK(needs_owner(Standing::NOT_RECOGNISED));
  for (Standing s : {Standing::NO_ROOT, Standing::ASKING, Standing::PAIRED, Standing::RENEWING, Standing::EXPIRED,
                     Standing::CLOCK_NOT_SET})
    CHECK(!needs_owner(s));
}

TEST(only_a_display_that_prefers_https_falls_back_after_a_failure) {
  CHECK(falls_back(Transport::PREFER_HTTPS));
  CHECK(!falls_back(Transport::HTTPS));
  CHECK(!falls_back(Transport::HTTP));
}

TEST(a_refused_certificate_is_its_own_reason_and_anything_else_is_the_server_or_the_download) {
  CHECK(failure_after(Start::REFUSED) == Failure::CERTIFICATE);
  CHECK(failure_after(Start::UNREACHABLE) == Failure::SERVER);
  CHECK(failure_after(Start::BAD_REPLY) == Failure::SERVER);
  CHECK(download_failure(true, Start::REFUSED) == Failure::CERTIFICATE);
  CHECK(download_failure(true, Start::UNREACHABLE) == Failure::DOWNLOAD);
  CHECK(download_failure(false, Start::REFUSED) == Failure::DOWNLOAD);  // plain HTTP has no certificate to refuse
  CHECK(download_failure(false, Start::OK) == Failure::DOWNLOAD);
}

TEST(waiting_for_the_owner_sleeps_as_the_server_said_within_the_usual_bounds) {
  CHECK_EQ(wait_sleep_ms(300), 300000u);
  CHECK_EQ(wait_sleep_ms(0), 300000u);             // it said nothing
  CHECK_EQ(wait_sleep_ms(5), 60000u);              // never less than a minute
  CHECK_EQ(wait_sleep_ms(10 * 86400), 86400000u);  // never more than a day
}
