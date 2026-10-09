// Session resumption on the data connections (eink_session_cache.h, eink_tls.h, eink_stream.h) against the real server
// on a computer: the plan and the picture of a wake resume what the first connection left, a session survives being
// written out and read back as it does through deep sleep (with the age put right, which is the thing the chip's clock
// would otherwise break), and everything that makes a saved session unusable ends in a connection made in full, not a
// failed wake.
//
// Run through with_fixture.sh, which starts the server and says where it is.
#include <string>

#include "host_support.h"
#include "eink_secure.h"
#include "eink_session_cache.h"

using namespace support;
using eink_stream::Stream;
using Bytes = eink_session_cache::Bytes;

namespace {

// How the server says its handshakes began. The request that asks is itself a connection, made without a store so that
// it does not disturb what a test keeps; it is counted as a full one, in what it is told.
struct Counts {
  int full = 0, resumed = 0;
};

Counts server_counts(Display &d) {
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = nullptr;
  Stream stream;
  Counts counts;
  if (stream.start(peer, "GET", "/handshakes") != Stream::Start::OK)
    return counts;
  std::string body;
  stream.read_all(body, 256);
  std::sscanf(body.c_str(), "full=%d resumed=%d", &counts.full, &counts.resumed);
  return counts;
}

// A wall clock a test can move.
struct Wall {
  int64_t now = 1791463200;
  int64_t operator()() const { return now; }
};

struct Cache {
  eink_session_cache::Slot slot;
  Wall wall;
  eink_session_cache::SlotStore<std::reference_wrapper<Wall>> store;
  Cache(Display &d)
      : store(eink_session_cache::make_store(
            slot,
            eink_session_cache::make_key(server_ip(), server_port(), d.identity.stored, d.identity.certificate_der),
            std::ref(wall))) {
    eink_session_cache::clear(slot);
  }
};

bool get(Display &d, eink_stream::Peer peer, const std::string &path, std::string &body, int &status) {
  Stream stream;
  if (stream.start(peer, "GET", path) != Stream::Start::OK)
    return false;
  status = stream.status();
  return stream.read_all(body, 1 << 20);
}

}  // namespace

TEST(the_second_connection_of_a_wake_resumes_the_first_and_is_still_known_by_its_certificate) {
  Display d("host-resume-wake");
  join_as(d);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;

  const Counts before = server_counts(d);
  std::string plan, image, who;
  int status = 0;
  CHECK(get(d, peer, "/plan", plan, status));  // the first: in full, and it leaves a ticket
  CHECK(cache.slot.length > 0);
  CHECK(get(d, peer, "/big/50000", image, status));  // the picture: resumes
  CHECK(get(d, peer, "/who", who, status));
  CHECK_EQ(who, "host-resume-wake");  // a resumed connection names the display by the certificate of the first
  const Counts after = server_counts(d);
  CHECK_EQ(after.resumed - before.resumed, 2);
  CHECK_EQ(image.size(), (size_t) 50000);
}

TEST(a_session_survives_being_written_out_and_read_back_as_after_deep_sleep) {
  Display d("host-resume-sleep");
  join_as(d);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/plan", body, status));

  // The chip sleeps ten minutes: the program starts again with nothing but the slot (RTC memory) and the wall clock.
  eink_session_cache::Slot kept = cache.slot;
  Cache woken(d);
  woken.slot = kept;
  woken.wall.now = cache.wall.now + 600;
  eink_stream::Peer again = peer_of(d);
  again.sessions = &woken.store;

  const Counts before = server_counts(d);
  body.clear();
  CHECK(get(d, again, "/who", body, status));
  CHECK_EQ(body, "host-resume-sleep");
  CHECK_EQ(server_counts(d).resumed - before.resumed, 1);
}

TEST(a_ticket_whose_clock_started_again_is_still_accepted_because_its_age_is_put_right) {
  Display d("host-resume-clock");
  join_as(d);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/plan", body, status));

  // Make the saved session look as one from before a deep sleep does: its reception time is that of a clock that was
  // further on than this one is now, so by mbedTLS's own arithmetic it came from the future and it would be refused.
  Bytes saved;
  int64_t age = 0;
  CHECK(cache.store.load(saved, age));
  mbedtls_ssl_session session;
  mbedtls_ssl_session_init(&session);
  CHECK_EQ(mbedtls_ssl_session_load(&session, saved.data(), saved.size()), 0);
  session.MBEDTLS_PRIVATE(ticket_reception_time) = mbedtls_ms_time() + 3600 * 1000;  // an hour ahead
  const Bytes skewed = eink_tls::Session::serialise(session);
  mbedtls_ssl_session_free(&session);
  CHECK(!skewed.empty());
  cache.store.save(skewed);

  // Offered as it is, it would not be accepted: that is what the chip's clock would do without the age being put right.
  mbedtls_ssl_session raw;
  mbedtls_ssl_session_init(&raw);
  CHECK_EQ(mbedtls_ssl_session_load(&raw, skewed.data(), skewed.size()), 0);
  CHECK(mbedtls_ms_time() - raw.MBEDTLS_PRIVATE(ticket_reception_time) < 0);
  mbedtls_ssl_session_free(&raw);

  // Through the cache the age is put right (this is what `restore` does), so it resumes.
  const Counts before = server_counts(d);
  body.clear();
  CHECK(get(d, peer, "/who", body, status));
  CHECK_EQ(body, "host-resume-clock");
  CHECK_EQ(server_counts(d).resumed - before.resumed, 1);
}

TEST(a_ticket_the_server_does_not_know_is_a_full_handshake_not_a_failed_request) {
  Display d("host-resume-unknown");
  join_as(d);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/plan", body, status));

  // The server was restarted with other ticket keys: what it sealed is now unreadable to it. Damage the ticket.
  Bytes saved;
  int64_t age = 0;
  CHECK(cache.store.load(saved, age));
  for (size_t at = saved.size() - 60; at < saved.size() - 20; at++)
    saved[at] ^= 0x5A;
  cache.store.save(saved);

  const Counts before = server_counts(d);
  body.clear();
  CHECK(get(d, peer, "/who", body, status));
  CHECK_EQ(body, "host-resume-unknown");  // it worked, in full
  CHECK_EQ(server_counts(d).resumed - before.resumed, 0);

  // And the connection after it resumes the new ticket that one left.
  const Counts mid = server_counts(d);
  body.clear();
  CHECK(get(d, peer, "/who", body, status));
  CHECK_EQ(server_counts(d).resumed - mid.resumed, 1);
}

TEST(a_session_the_library_cannot_read_is_forgotten_and_the_connection_made_in_full) {
  Display d("host-resume-garbage");
  join_as(d);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  cache.store.save(Bytes(400, 0xAB));  // not a session
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/who", body, status));
  CHECK_EQ(body, "host-resume-garbage");
}

TEST(a_renewed_certificate_does_not_use_the_session_of_the_one_before) {
  Display d("host-resume-renew");
  join_as(d);
  Cache before_renewal(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &before_renewal.store;
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/plan", body, status));
  const eink_session_cache::Slot kept = before_renewal.slot;

  d.clock.offset = 61 * DAY;
  CHECK(d.wake().paired);  // renews: a new certificate

  Cache after(d);  // its key is made from the new certificate
  after.slot = kept;
  Bytes saved;
  int64_t age;
  CHECK(!after.store.load(saved, age));  // another key: not offered
}

TEST(sessions_are_not_shared_between_displays) {
  Display a("host-resume-a"), b("host-resume-b");
  join_as(a);
  join_as(b);
  Cache cache_a(a);
  eink_stream::Peer peer_a = peer_of(a);
  peer_a.sessions = &cache_a.store;
  std::string body;
  int status = 0;
  CHECK(get(a, peer_a, "/plan", body, status));

  Cache cache_b(b);
  cache_b.slot = cache_a.slot;  // b is handed a's memory
  Bytes saved;
  int64_t age;
  CHECK(!cache_b.store.load(saved, age));
}

TEST(a_display_the_owner_revoked_is_still_turned_away_on_a_resumed_connection) {
  Display d("host-resume-revoked");
  Display observer("host-resume-observer");  // counts the server's handshakes, which a revoked display cannot ask for
  join_as(d);
  join_as(observer);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/who", body, status));
  CHECK_EQ(status, 200);
  CHECK_EQ(ctl("revoke host-resume-revoked"), 0);
  const Counts before = server_counts(observer);
  body.clear();
  CHECK(get(d, peer, "/who", body, status));
  CHECK_EQ(status, 403);  // membership is checked on every request, resumed or not
  CHECK_EQ(server_counts(observer).resumed - before.resumed, 1);
}

TEST(with_no_store_every_connection_is_in_full) {
  Display d("host-resume-none");
  join_as(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = nullptr;
  const Counts before = server_counts(d);
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/plan", body, status));
  CHECK(get(d, peer, "/plan", body, status));
  CHECK_EQ(server_counts(d).resumed - before.resumed, 0);
}

TEST(a_session_with_the_clock_not_believed_is_neither_kept_nor_offered) {
  Display d("host-resume-noclock");
  join_as(d);
  Cache cache(d);
  cache.wall.now = -1;  // not set by SNTP yet
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  std::string body;
  int status = 0;
  const Counts before = server_counts(d);
  CHECK(get(d, peer, "/plan", body, status));
  CHECK(get(d, peer, "/plan", body, status));
  CHECK_EQ(cache.slot.length, 0u);
  CHECK_EQ(server_counts(d).resumed - before.resumed, 0);
}

TEST(what_a_session_costs_in_memory_and_in_bytes_on_the_wire) {
  Display d("host-resume-size");
  join_as(d);
  Cache cache(d);
  eink_stream::Peer peer = peer_of(d);
  peer.sessions = &cache.store;
  std::string body;
  int status = 0;
  CHECK(get(d, peer, "/plan", body, status));
  std::printf("      a saved session is %u bytes (room for %zu)\n", cache.slot.length, eink_session_cache::CAPACITY);
  CHECK(cache.slot.length > 0);
  // Half is the headroom the comment on CAPACITY promises: a longer chain or certificate must not fill the slot.
  CHECK(cache.slot.length <= eink_session_cache::CAPACITY / 2);
}
