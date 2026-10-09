// A wake's beginning (tls/secure_begin.h) against the real server on a computer, for each way the display can be set:
// joining, deciding the route, and the first request through the connection it leaves behind. The display's flash is a
// stand-in that keeps what it is given, and each wake starts afresh, as on the chip.
//
// Run through with_fixture.sh, which starts the server and says where it is.
#include <memory>
#include <string>

#include "host_support.h"
#include "eink/tls/secure.h"
#include "eink/tls/secure_begin.h"

using namespace support;
using eink_secure_begin::Begin;
using eink_secure_wake::Route;
using eink_service::Transport;

namespace {

// A display that starts afresh at every wake and keeps what the chip keeps.
struct Wakes {
  std::string name;
  fakes::MemoryStore flash;
  eink_pairing::Answer rtc = eink_pairing::Answer::NONE;
  HostClock clock;
  eink_ports::Bytes compiled;

  explicit Wakes(const std::string &n) : name(n) {}

  // The identity holds the key that tls/secure.h points at, so it lives until the next wake begins, as the chip's does
  // (a static there).
  std::unique_ptr<eink_flash::FlashIdentity> identity;

  Begin begin(Transport transport, uint32_t ip = server_ip(), uint16_t tls_port = server_port()) {
    eink_secure::forget();
    identity.reset(new eink_flash::FlashIdentity(flash, rtc, compiled));
    return eink_secure_begin::begin(transport, ip, tls_port, name, clock, *identity);
  }
};

}  // namespace

TEST(plain_http_joins_nothing_and_opens_no_secure_connection) {
  Wakes w("host-begin-http");
  const Begin b = w.begin(Transport::HTTP);
  CHECK(b.verdict.route == Route::PLAIN && b.verdict.failure == Failure::NONE && !b.verdict.prompt);
  CHECK(!eink_secure::context().ready);
  CHECK(w.flash.blobs.empty());  // nothing written
}

TEST(https_only_waits_for_the_owner_shows_the_code_and_then_speaks_tls) {
  Wakes w("host-begin-https");
  const Begin waiting = w.begin(Transport::HTTPS);
  CHECK(waiting.verdict.route == Route::WAIT_FOR_OWNER);
  CHECK(waiting.verdict.failure == Failure::APPROVAL);
  CHECK(waiting.verdict.prompt);
  CHECK_EQ(waiting.retry_after_s, 300u);
  CHECK(waiting.prompt.find("Waiting for approval: ") == 0);
  CHECK(!eink_secure::context().ready);

  // The owner types in the code the panel shows.
  const std::string code = waiting.prompt.substr(std::string("Waiting for approval: ").size());
  CHECK_EQ(ctl("approve host-begin-https " + code), 0);

  const Begin secure = w.begin(Transport::HTTPS);
  CHECK(secure.verdict.route == Route::SECURE);
  CHECK(!secure.verdict.prompt);
  CHECK(eink_secure::context().ready);
  // And the first request through what it left behind: the server knows the display by its certificate.
  const eink_secure::Fetched who = eink_secure::fetch("GET", "/who");
  CHECK(who.ok());
  CHECK_EQ(who.body, "host-begin-https");
  eink_secure::forget();
}

TEST(a_display_that_prefers_https_goes_on_over_plain_http_while_it_waits_and_still_shows_the_code) {
  Wakes w("host-begin-prefer");
  const Begin waiting = w.begin(Transport::PREFER_HTTPS);
  CHECK(waiting.verdict.route == Route::PLAIN);
  CHECK(waiting.verdict.failure == Failure::APPROVAL);
  CHECK(waiting.verdict.prompt);
  CHECK(!waiting.prompt.empty());
  CHECK(!eink_secure::context().ready);
  const std::string code = waiting.prompt.substr(std::string("Waiting for approval: ").size());
  CHECK_EQ(ctl("approve host-begin-prefer " + code), 0);
  CHECK(w.begin(Transport::PREFER_HTTPS).verdict.route == Route::SECURE);
  eink_secure::forget();
}

TEST(a_server_nobody_answers_is_a_failed_wake_for_https_and_a_fallback_for_prefer_https) {
  Wakes only("host-begin-gone-a");
  const Begin fails = only.begin(Transport::HTTPS, server_ip(), 1);
  CHECK(fails.verdict.route == Route::FAIL && fails.verdict.failure == Failure::SERVER);

  Wakes prefer("host-begin-gone-b");
  const Begin falls = prefer.begin(Transport::PREFER_HTTPS, server_ip(), 1);
  CHECK(falls.verdict.route == Route::PLAIN && falls.verdict.failure == Failure::SERVER && !falls.verdict.prompt);
}

TEST(a_server_that_announces_no_https_port_is_not_asked_anything) {
  Wakes only("host-begin-noport-a");
  const Begin fails = only.begin(Transport::HTTPS, server_ip(), 0);
  CHECK(fails.verdict.route == Route::FAIL && fails.verdict.failure == Failure::SERVER);
  CHECK(only.flash.blobs.empty());  // no key made, nothing kept: it never tried to join

  Wakes prefer("host-begin-noport-b");
  const Begin falls = prefer.begin(Transport::PREFER_HTTPS, 0, 0);  // not even found
  CHECK(falls.verdict.route == Route::PLAIN && falls.verdict.failure == Failure::SERVER);
}

TEST(a_root_built_in_that_is_not_the_servers_fails_https_with_a_certificate_and_prefers_plain) {
  Wakes only("host-begin-wrongroot-a");
  only.compiled = someone_elses_root();
  const Begin fails = only.begin(Transport::HTTPS);
  CHECK(fails.verdict.route == Route::FAIL && fails.verdict.failure == Failure::CERTIFICATE);
  CHECK(!eink_secure::context().ready);

  Wakes prefer("host-begin-wrongroot-b");
  prefer.compiled = someone_elses_root();
  const Begin falls = prefer.begin(Transport::PREFER_HTTPS);
  CHECK(falls.verdict.route == Route::PLAIN && falls.verdict.failure == Failure::CERTIFICATE);
}

TEST(a_paired_display_begins_without_asking_the_enrolment_anything) {
  Wakes w("host-begin-steady");
  const Begin first = w.begin(Transport::HTTPS);
  const std::string code = first.prompt.substr(std::string("Waiting for approval: ").size());
  CHECK_EQ(ctl("approve host-begin-steady " + code), 0);
  CHECK(w.begin(Transport::HTTPS).verdict.route == Route::SECURE);
  const int writes = w.flash.writes;
  CHECK(w.begin(Transport::HTTPS).verdict.route == Route::SECURE);
  CHECK_EQ(w.flash.writes, writes);  // an ordinary wake writes nothing
  eink_secure::forget();
}

TEST(after_the_owner_revokes_it_a_display_is_asked_for_the_code_again) {
  Wakes w("host-begin-revoked");
  const Begin first = w.begin(Transport::HTTPS);
  CHECK_EQ(ctl("approve host-begin-revoked " + first.prompt.substr(std::string("Waiting for approval: ").size())), 0);
  CHECK(w.begin(Transport::HTTPS).verdict.route == Route::SECURE);
  CHECK_EQ(ctl("revoke host-begin-revoked"), 0);
  w.clock.offset = 70 * DAY;  // due to renew, so the server is asked and refuses
  const Begin again = w.begin(Transport::HTTPS);
  CHECK(again.verdict.route == Route::WAIT_FOR_OWNER);
  CHECK(again.verdict.failure == Failure::UNRECOGNISED);
  CHECK(again.prompt.find("Not recognised") == 0);
  eink_secure::forget();
}

TEST(a_wake_that_begins_with_a_session_store_leaves_the_plan_and_the_picture_to_resume) {
  Wakes w("host-begin-resume");
  const Begin first = w.begin(Transport::HTTPS);
  CHECK_EQ(ctl("approve host-begin-resume " + first.prompt.substr(std::string("Waiting for approval: ").size())), 0);

  eink_session_cache::Slot slot;
  eink_session_cache::clear(slot);
  int64_t wall = 1791463200;
  auto holder = eink_session_cache::make_store(slot, eink_session_cache::Key{}, [&wall] { return wall; });

  w.identity.reset(new eink_flash::FlashIdentity(w.flash, w.rtc, w.compiled));
  const Begin secure =
      eink_secure_begin::begin(Transport::HTTPS, server_ip(), server_port(), w.name, w.clock, *w.identity, &holder);
  CHECK(secure.verdict.route == Route::SECURE);
  const eink_secure::Fetched plan = eink_secure::fetch("GET", "/plan");
  CHECK(plan.ok());
  CHECK(slot.length > 0);  // the plan's connection left a ticket
  const eink_secure::Fetched who = eink_secure::fetch("GET", "/who");
  CHECK_EQ(who.body, "host-begin-resume");

  // A wake later, after a sleep: a new begin picks the store of the same server and identity, and the slot is still
  // good.
  wall += 600;
  w.identity.reset(new eink_flash::FlashIdentity(w.flash, w.rtc, w.compiled));
  const Begin again =
      eink_secure_begin::begin(Transport::HTTPS, server_ip(), server_port(), w.name, w.clock, *w.identity, &holder);
  CHECK(again.verdict.route == Route::SECURE);
  const uint32_t length = slot.length;
  CHECK(length > 0);
  CHECK(eink_secure::fetch("GET", "/who").ok());

  // The server's own count, from a server this test has to itself: the enrolment connections (never resumed) and the
  // plan were in full; the request for who it is, the same request a wake later, and this one all resumed.
  const eink_secure::Fetched counts = eink_secure::fetch("GET", "/handshakes");
  CHECK(counts.ok());
  int full = 0, resumed = 0;
  std::sscanf(counts.body.c_str(), "full=%d resumed=%d", &full, &resumed);
  CHECK_EQ(resumed, 3);
  CHECK(full >= 4);  // the enrolment ones and the plan
  eink_secure::forget();
}
