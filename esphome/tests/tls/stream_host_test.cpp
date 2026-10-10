// The firmware's streaming client (tls/stream.h) against the real server on a computer: a body of megabytes read in
// pieces of whatever size, in chunks or with a length, a connection that goes quiet, and a display the server turns
// away.
//
// Run through with_fixture.sh, which starts the server and says where it is.
#include <chrono>
#include <string>

#include "host_support.h"
#include "home_display/tls/secure.h"

using namespace support;
using home_display_stream::Stream;

namespace {

// Every byte of a body made of i % 251, read in pieces of `piece` bytes.
bool reads_pattern(Stream &stream, size_t total, size_t piece) {
  std::vector<uint8_t> buffer(piece);
  size_t at = 0;
  for (;;) {
    const int n = stream.read(buffer.data(), buffer.size());
    if (n < 0)
      return false;
    if (n == 0)
      break;
    for (int i = 0; i < n; i++)
      if (buffer[static_cast<size_t>(i)] != static_cast<uint8_t>((at + static_cast<size_t>(i)) % 251))
        return false;
    at += static_cast<size_t>(n);
  }
  return at == total;
}

}  // namespace

TEST(a_plan_is_fetched_over_a_verified_connection_that_shows_the_certificate) {
  Display d("host-stream-plan");
  join_as(d);
  Stream stream;
  CHECK(stream.start(peer_of(d), "GET", "/plan") == Stream::Start::OK);
  CHECK_EQ(stream.status(), 200);
  CHECK_EQ(stream.header("content-type"), "application/json");
  std::string body;
  CHECK(stream.read_all(body, 4096));
  CHECK(body.find("\"next_seconds\":600") != std::string::npos);
  CHECK(stream.finished());
}

TEST(a_body_of_megabytes_with_a_length_arrives_whole_in_pieces_of_any_size) {
  Display d("host-stream-big");
  join_as(d);
  const size_t total = 2600000;  // about the size of the 1872 x 1404 greyscale image
  for (size_t piece : {1460u, 4096u, 16384u}) {
    Stream stream;
    CHECK(stream.start(peer_of(d), "GET", "/big/" + std::to_string(total)) == Stream::Start::OK);
    CHECK_EQ(stream.status(), 200);
    CHECK(stream.has_length());
    CHECK_EQ(stream.content_length(), total);
    CHECK(reads_pattern(stream, total, piece));
    CHECK(stream.finished());
    CHECK_EQ(stream.bytes_read(), total);
  }
}

TEST(a_body_sent_in_chunks_arrives_whole_too) {
  Display d("host-stream-chunked");
  join_as(d);
  const size_t total = 300000;
  Stream stream;
  CHECK(stream.start(peer_of(d), "GET", "/chunked/" + std::to_string(total)) == Stream::Start::OK);
  CHECK(!stream.has_length());
  CHECK(reads_pattern(stream, total, 2048));
  CHECK(stream.finished());
}

TEST(an_empty_body_is_finished_at_once) {
  Display d("host-stream-empty");
  join_as(d);
  Stream stream;
  CHECK(stream.start(peer_of(d), "GET", "/big/0") == Stream::Start::OK);
  uint8_t byte;
  CHECK_EQ(stream.read(&byte, 1), 0);
  CHECK(stream.finished());
}

TEST(a_connection_that_goes_quiet_is_an_error_and_not_a_wait_for_ever) {
  Display d("host-stream-stall");
  join_as(d);
  Stream stream;
  home_display_stream::Peer peer = peer_of(d);
  peer.timeout_ms = 1500;
  CHECK(stream.start(peer, "GET", "/stall") == Stream::Start::OK);
  std::string body;
  CHECK(!stream.read_all(body, 1000));  // "some", then nothing
  CHECK_EQ(body, "some");
  CHECK(!stream.finished());
}

TEST(a_path_the_server_does_not_have_is_a_status_and_not_a_failure) {
  Display d("host-stream-404");
  join_as(d);
  Stream stream;
  CHECK(stream.start(peer_of(d), "GET", "/nothing-here") == Stream::Start::OK);
  CHECK_EQ(stream.status(), 404);
}

TEST(a_display_the_server_has_revoked_is_turned_away) {
  Display d("host-stream-revoked");
  join_as(d);
  CHECK_EQ(ctl("revoke host-stream-revoked"), 0);
  Stream stream;
  const Stream::Start started = stream.start(peer_of(d), "GET", "/who");
  // The handshake is let through (the certificate is still valid); the request is refused by name, per request.
  CHECK(started == Stream::Start::OK);
  CHECK_EQ(stream.status(), 403);
}

TEST(a_server_that_does_not_chain_to_the_root_is_refused_before_anything_is_sent) {
  Display d("host-stream-middle");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);
  peer.root = someone_elses_root();
  Stream stream;
  CHECK(stream.start(peer, "GET", "/image") == Stream::Start::REFUSED);
}

TEST(nothing_listening_is_unreachable) {
  Display d("host-stream-gone");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);
  peer.port = 1;
  Stream stream;
  CHECK(stream.start(peer, "GET", "/image") == Stream::Start::UNREACHABLE);
}

TEST(a_stream_can_be_used_again_after_it_is_closed) {
  Display d("host-stream-again");
  join_as(d);
  Stream stream;
  for (int i = 0; i < 3; i++) {
    CHECK(stream.start(peer_of(d), "GET", "/big/5000") == Stream::Start::OK);
    CHECK(reads_pattern(stream, 5000, 700));
    stream.close();
  }
}

TEST(a_small_request_is_fetched_whole_with_its_status_and_headers) {
  Display d("host-fetch");
  join_as(d);
  home_display_secure::use(server_ip(), server_port(), d.identity.stored, d.identity.certificate_der,
                           &d.identity.private_key());
  const home_display_secure::Fetched plan = home_display_secure::fetch("GET", "/plan?have=1&device=host-fetch");
  CHECK(plan.ok());
  CHECK_EQ(plan.status, 200);
  CHECK(plan.body.find("\"utc_offset_seconds\":3600") != std::string::npos);

  const home_display_secure::Fetched who = home_display_secure::fetch("GET", "/who");
  CHECK_EQ(who.body, "host-fetch");  // known by its certificate

  const home_display_secure::Fetched missing = home_display_secure::fetch("GET", "/nothing");
  CHECK(missing.ok());
  CHECK_EQ(missing.status, 404);
  home_display_secure::forget();
}

TEST(a_body_longer_than_the_limit_is_not_a_plan) {
  Display d("host-fetch-long");
  join_as(d);
  home_display_secure::use(server_ip(), server_port(), d.identity.stored, d.identity.certificate_der,
                           &d.identity.private_key());
  const home_display_secure::Fetched big = home_display_secure::fetch("GET", "/big/100000");
  CHECK(!big.ok());
  CHECK(big.body.empty());
  home_display_secure::forget();
}

TEST(nothing_is_sent_until_the_script_has_said_where_the_server_is) {
  home_display_secure::forget();
  const home_display_secure::Fetched none = home_display_secure::fetch("GET", "/plan");
  CHECK(!none.ok());
  CHECK(none.start == Stream::Start::UNREACHABLE);
}

TEST(how_the_last_request_got_on_is_kept_for_the_script_to_classify) {
  Display d("host-fetch-class");
  join_as(d);
  home_display_secure::use(server_ip(), server_port(), d.identity.stored, d.identity.certificate_der,
                           &d.identity.private_key());
  home_display_secure::fetch("GET", "/plan");
  CHECK(home_display_secure::context().last == Stream::Start::OK);
  home_display_secure::use(server_ip(), server_port(), someone_elses_root(), d.identity.certificate_der,
                           &d.identity.private_key());
  home_display_secure::fetch("GET", "/plan");
  CHECK(home_display_secure::context().last == Stream::Start::REFUSED);
  home_display_secure::use(server_ip(), 1, d.identity.stored, d.identity.certificate_der, &d.identity.private_key());
  home_display_secure::fetch("GET", "/plan");
  CHECK(home_display_secure::context().last == Stream::Start::UNREACHABLE);
  home_display_secure::forget();
}

TEST(the_server_can_be_moved_without_losing_what_the_display_holds) {
  Display d("host-aim");
  join_as(d);
  home_display_secure::use(server_ip(), 1, d.identity.stored, d.identity.certificate_der, &d.identity.private_key());
  CHECK(!home_display_secure::fetch("GET", "/plan").ok());  // nothing at port 1
  home_display_secure::aim(server_ip(), server_port());
  CHECK(home_display_secure::fetch("GET", "/plan").ok());
  home_display_secure::aim(0, 0);
  CHECK(!home_display_secure::context().ready);
  home_display_secure::forget();
}

TEST(a_small_request_refuses_a_reply_bigger_than_its_limit) {
  Display d("host-stream-limit");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);
  const auto parsed = home_display_stream::parse(peer);
  const auto ask = [&](size_t limit, const char *path) {
    home_display_tls::Session session;
    CHECK(session.open(peer.ip, peer.port, &parsed->trust, &parsed->own, peer.key) == home_display_tls::Open::OK);
    return session.request("GET", path, "", "", limit);
  };
  // Within the limit it is the answer; over it there is none, rather than a heap filled by whatever the server sends.
  const home_display_http::Response fits = ask(64 * 1024, "/big/20000");
  CHECK_EQ(fits.status, 200);
  CHECK_EQ(fits.body.size(), 20000u);
  CHECK_EQ(ask(4 * 1024, "/big/20000").status, 0);
}

TEST(a_server_that_never_stops_trickling_is_given_up_on_at_the_deadline) {
  Display d("host-stream-trickle");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);
  peer.timeout_ms = 1500;  // every wait is far shorter than this: a byte comes every 100 ms
  peer.deadline_ms = 1000;
  Stream stream;
  const auto began = std::chrono::steady_clock::now();
  CHECK(stream.start(peer, "GET", "/trickle/100") == Stream::Start::OK);  // 10 s of body
  std::string body;
  CHECK(!stream.read_all(body, 1000));
  const auto took = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - began);
  CHECK(took.count() >= 900);
  CHECK(took.count() < 3000);
  CHECK(!body.empty() && body.size() < 100);  // what arrived before it ended
  CHECK(!stream.finished());
}

TEST(a_deadline_given_for_one_request_replaces_the_peers) {
  Display d("host-stream-timing");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);
  peer.deadline_ms = 60000;
  Stream stream;
  const auto began = std::chrono::steady_clock::now();
  CHECK(stream.start(peer, "GET", "/trickle/100", {}, "", "", {1500, 700}) == Stream::Start::OK);
  std::string body;
  CHECK(!stream.read_all(body, 1000));
  const auto took = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - began);
  CHECK(took.count() < 2500);
}

TEST(a_request_within_its_deadline_is_unaffected) {
  Display d("host-stream-inside");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);
  peer.deadline_ms = 30000;
  Stream stream;
  CHECK(stream.start(peer, "GET", "/big/50000") == Stream::Start::OK);
  std::string body;
  CHECK(stream.read_all(body, 1 << 20));
  CHECK_EQ(body.size(), 50000u);
  CHECK(stream.finished());
}

TEST(a_picture_the_server_says_the_display_has_is_a_304_with_no_body) {
  Display d("host-stream-304");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);

  // Asked without, it is sent whole, with the name of its bytes.
  Stream first;
  CHECK(first.start(peer, "GET", "/etagged") == Stream::Start::OK);
  CHECK_EQ(first.status(), 200);
  CHECK_EQ(first.header("ETag"), "\"v1\"");
  std::string body;
  CHECK(first.read_all(body, 100));
  CHECK_EQ(body, "picture");

  // Asked with that name, there is nothing to read, and it is over at once.
  Stream second;
  CHECK(second.start(peer, "GET", "/etagged", {{"If-None-Match", first.header("etag")}}) == Stream::Start::OK);
  CHECK_EQ(second.status(), 304);
  CHECK_EQ(second.header("etag"), "\"v1\"");
  uint8_t byte;
  CHECK_EQ(second.read(&byte, 1), 0);
  CHECK(second.finished());

  // Another name is a picture again.
  Stream third;
  CHECK(third.start(peer, "GET", "/etagged", {{"If-None-Match", "\"v0\""}}) == Stream::Start::OK);
  CHECK_EQ(third.status(), 200);
}

TEST(a_connection_says_how_long_its_handshake_took_and_the_wake_keeps_the_first) {
  Display d("host-stream-handshake");
  join_as(d);
  home_display_stream::Peer peer = peer_of(d);

  Stream stream;
  CHECK_EQ(stream.handshake_ms(), 0u);  // nothing yet
  CHECK(stream.start(peer, "GET", "/who") == Stream::Start::OK);
  CHECK(stream.handshake_ms() >= 1 && stream.handshake_ms() < 10000);
  stream.close();
  CHECK_EQ(stream.handshake_ms(), 0u);

  // Through the wake's own entry: the first connection's time is kept, and the next does not replace it.
  const home_display_stream::Peer root = peer_of(d);
  home_display_secure::use(root.ip, root.port, root.root, root.certificate, root.key);
  CHECK_EQ(home_display_secure::context().first_handshake_ms, 0u);
  CHECK(home_display_secure::fetch("GET", "/who").ok());
  const uint32_t first = home_display_secure::context().first_handshake_ms;
  CHECK(first >= 1 && first < 10000);
  home_display_secure::context().first_handshake_ms = first + 12345;  // so a replacement would show
  CHECK(home_display_secure::fetch("GET", "/who").ok());
  CHECK_EQ(home_display_secure::context().first_handshake_ms, first + 12345);
  home_display_secure::forget();
  CHECK_EQ(home_display_secure::context().first_handshake_ms, 0u);
}
