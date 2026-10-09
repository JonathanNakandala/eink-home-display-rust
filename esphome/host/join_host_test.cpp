// The firmware's own joining code (eink_join.h) over its own TLS and EST code (eink_est_client.h and the rest), run
// against a real server on a computer, with the same mbedTLS the chip is built with. Each test is a display that joins,
// renews, or is turned away, as it would on the chip but for the flash, the radio and the clock being the computer's.
//
// Run through with_fixture.sh, which starts the server and says where it is.
#include <arpa/inet.h>
#include <cstdlib>
#include <ctime>
#include <string>

#include "mbedtls/x509_crt.h"
#include "mbedtls/x509_csr.h"

#include "check.h"
#include "memory_identity.h"
#include "eink_est_client.h"
#include "eink_join.h"
#include "eink_verifier.h"

using eink_join::Joiner;
using eink_join::Outcome;
using eink_pairing::Standing;
using eink_report::Failure;
using host::MemoryIdentity;

namespace {

constexpr int64_t DAY = 86400;

std::string environment(const char *name) {
  const char *value = std::getenv(name);
  if (value == nullptr) {
    std::fprintf(stderr, "%s is not set: run through host/with_fixture.sh\n", name);
    std::exit(2);
  }
  return value;
}

uint16_t server_port() { return static_cast<uint16_t>(std::atoi(environment("FIXTURE_PORT").c_str())); }
uint32_t server_ip() { return inet_addr("127.0.0.1"); }  // network order: the first octet in the lowest byte, as lwIP

// The computer's clock, which a test can move forward to be at a certificate's third, or past its end.
struct HostClock : eink_ports::Clock {
  int64_t offset = 0;
  int64_t now() override { return static_cast<int64_t>(std::time(nullptr)) + offset; }
  bool usable() override { return true; }
};

// The real client, counted.
struct CountingEst : eink_ports::Est {
  eink_est::EstClient inner;
  int roots = 0, enrolled = 0, renewed = 0;
  CountingEst(uint32_t ip, uint16_t port, eink_tls::TlsIdentity &identity) : inner(ip, port, identity) {}
  eink_ports::RootReply fetch_root() override {
    roots++;
    return inner.fetch_root();
  }
  eink_ports::Reply enroll(const std::string &n, eink_ports::Identity &i, const eink_ports::Bytes &r) override {
    enrolled++;
    return inner.enroll(n, i, r);
  }
  eink_ports::Reply renew(const std::string &n, eink_ports::Identity &i, const eink_ports::Bytes &r) override {
    renewed++;
    return inner.renew(n, i, r);
  }
  int requests() const { return roots + enrolled + renewed; }
};

// A display: its identity, the clock it goes by, the real client and the real check.
struct Display {
  std::string name;
  MemoryIdentity identity;
  HostClock clock;
  CountingEst est;
  eink_verifier::MbedVerifier verifier;

  explicit Display(const std::string &n, uint16_t port = server_port()) : name(n), est(server_ip(), port, identity) {}
  Outcome wake() { return Joiner(clock, identity, est, verifier, name).run(); }
};

int ctl(const std::string &arguments) {
  const std::string command =
      environment("DISPLAYCTL") + " --socket " + environment("FIXTURE_ADMIN") + " " + arguments + " > /dev/null 2>&1";
  return std::system(command.c_str());
}

// A certificate that names itself and nobody else vouches for: someone in the middle's root.
eink_ports::Bytes someone_elses_root() {
  mbedtls_pk_context key;
  mbedtls_pk_init(&key);
  mbedtls_pk_setup(&key, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY));
  mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(key), eink_tls::Random::generate, nullptr);
  mbedtls_x509write_cert crt;
  mbedtls_x509write_crt_init(&crt);
  mbedtls_x509write_crt_set_subject_key(&crt, &key);
  mbedtls_x509write_crt_set_issuer_key(&crt, &key);
  mbedtls_x509write_crt_set_subject_name(&crt, "CN=someone else");
  mbedtls_x509write_crt_set_issuer_name(&crt, "CN=someone else");
  mbedtls_x509write_crt_set_version(&crt, MBEDTLS_X509_CRT_VERSION_3);
  mbedtls_x509write_crt_set_md_alg(&crt, MBEDTLS_MD_SHA256);
  mbedtls_x509write_crt_set_validity(&crt, "20250101000000", "20350101000000");
  mbedtls_x509write_crt_set_basic_constraints(&crt, 1, -1);
  unsigned char serial[] = {1};
  mbedtls_x509write_crt_set_serial_raw(&crt, serial, sizeof serial);
  unsigned char buffer[1024];
  const int n = mbedtls_x509write_crt_der(&crt, buffer, sizeof buffer, eink_tls::Random::generate, nullptr);
  mbedtls_x509write_crt_free(&crt);
  mbedtls_pk_free(&key);
  return n > 0 ? eink_ports::Bytes(buffer + sizeof buffer - n, buffer + sizeof buffer) : eink_ports::Bytes();
}

// Takes a display from nothing to a certificate, with the owner approving by the code on the panel.
void join(Display &d) {
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK_EQ(ctl("approve " + d.name + " " + waiting.code), 0);
  const Outcome paired = d.wake();
  CHECK(paired.paired);
}

}  // namespace

TEST(a_new_display_waits_shows_the_code_the_server_approves_and_is_then_paired) {
  Display d("host-new");
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK(waiting.failure == Failure::APPROVAL);
  CHECK(!waiting.paired);
  CHECK_EQ(waiting.retry_after_s, 300u);
  CHECK_EQ(waiting.code.size(), (size_t) 14);
  CHECK(d.identity.has_key());
  CHECK(d.identity.stored.empty());  // no certificate has confirmed the root yet
  CHECK(!d.identity.has_cert);

  // The owner types the code from the panel in at the server; the server accepts it only if it worked out the same.
  CHECK_EQ(ctl("approve host-new " + waiting.code), 0);

  const Outcome paired = d.wake();
  CHECK(paired.paired);
  CHECK(paired.standing == Standing::PAIRED);
  CHECK(paired.failure == Failure::NONE);
  CHECK(!d.identity.stored.empty());  // kept now that a certificate has been issued under it
  CHECK(d.identity.has_cert);
  // The server dates it from an hour ago, to allow for a clock a little behind, and ends it 90 days from now.
  CHECK_EQ(d.identity.life.not_after - d.identity.life.not_before, 90 * DAY + 3600);
}

TEST(a_wrong_code_is_not_approved) {
  Display d("host-wrong");
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  std::string wrong = waiting.code;
  wrong[0] = wrong[0] == '0' ? '1' : '0';
  CHECK(ctl("approve host-wrong " + wrong) != 0);
  CHECK(d.wake().standing == Standing::WAITING);
}

TEST(a_paired_display_goes_on_without_asking_the_network_anything) {
  Display d("host-steady");
  join(d);
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome again = d.wake();
  CHECK(again.paired);
  CHECK_EQ(d.est.requests(), 0);
}

TEST(with_a_third_of_the_certificate_left_it_renews_with_no_owner) {
  Display d("host-renew");
  join(d);
  const eink_ports::Bytes first = d.identity.certificate_der;
  d.clock.offset = 61 * DAY;
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome renewed = d.wake();
  CHECK(renewed.paired);
  CHECK_EQ(d.est.renewed, 1);
  CHECK_EQ(d.est.enrolled, 0);
  CHECK(d.identity.certificate_der != first);  // a new certificate
}

TEST(a_certificate_that_ended_is_asked_for_again_with_the_same_key_and_no_owner) {
  Display d("host-expired");
  join(d);
  // The certificate the display holds ended ten days ago, by its own clock. (Moving the clock instead would make the
  // new certificate, which the server dates from the real time, look over as well.)
  const int64_t now = d.clock.now();
  d.identity.life = {now - 100 * DAY, now - 10 * DAY};
  const eink_ports::Bytes first = d.identity.certificate_der;
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome again = d.wake();
  CHECK(again.paired);
  CHECK_EQ(d.est.enrolled, 1);
  CHECK_EQ(d.est.roots, 0);
  CHECK_EQ(d.est.renewed, 0);
  CHECK(d.identity.certificate_der != first);
  CHECK(d.identity.life.not_after > now);
}

TEST(a_server_that_does_not_chain_to_the_pinned_root_is_refused_before_anything_is_sent) {
  Display d("host-middle");
  d.identity.compiled = someone_elses_root();
  CHECK(!d.identity.compiled.empty());
  const Outcome out = d.wake();
  CHECK(out.failure == Failure::CERTIFICATE);
  CHECK(!out.paired);
  CHECK(!d.identity.has_cert);
}

TEST(a_server_that_is_not_there_is_reported_as_the_server_and_changes_nothing) {
  Display d("host-gone", 1);  // nothing listens on port 1
  const Outcome out = d.wake();
  CHECK(out.failure == Failure::SERVER);
  CHECK(!d.identity.has_key());
  CHECK(d.identity.last == eink_pairing::Answer::NONE);
}

TEST(a_display_the_owner_has_revoked_is_not_kept_but_loses_nothing_it_holds) {
  Display d("host-revoked");
  join(d);
  CHECK_EQ(ctl("revoke host-revoked"), 0);
  d.clock.offset = 70 * DAY;  // due to renew
  const eink_ports::Bytes key_spki = d.identity.spki();
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome out = d.wake();
  // The server refuses the renewal, so it asks as a new display, and is refused that too: not recognised, and told so.
  CHECK_EQ(d.est.renewed, 1);
  CHECK_EQ(d.est.enrolled, 1);
  CHECK_EQ(d.est.roots, 0);
  CHECK(!out.paired);
  CHECK(out.standing == Standing::NOT_RECOGNISED);
  CHECK(out.failure == Failure::UNRECOGNISED);
  CHECK(d.identity.last == eink_pairing::Answer::REFUSED);
  // The panel has the code to give the owner.
  CHECK_EQ(out.code.size(), (size_t) 14);
  // Pairing is removed only by the owner: the key, the certificate and the root are all still there.
  CHECK(d.identity.has_key() && d.identity.has_cert && !d.identity.stored.empty());
  CHECK(d.identity.spki() == key_spki);
}

TEST(once_the_owner_approves_it_again_a_revoked_display_is_back) {
  Display d("host-back");
  join(d);
  CHECK_EQ(ctl("revoke host-back"), 0);
  d.clock.offset = 70 * DAY;
  const Outcome refused = d.wake();
  CHECK(refused.standing == Standing::NOT_RECOGNISED);
  // It was turned away; asked again (the window is open), the owner can now approve what it shows.
  CHECK_EQ(ctl("forget host-back"), 0);
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK_EQ(ctl("approve host-back " + waiting.code), 0);
  CHECK(d.wake().paired);
}
