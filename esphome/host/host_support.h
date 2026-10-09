// What the host tests share: where the real server is (from the environment with_fixture.sh sets), the computer's clock, the
// real EST client counted, a display held in memory, the admin commands, and taking a display from nothing to paired.
#pragma once

#include <arpa/inet.h>
#include <cstdlib>
#include <ctime>
#include <string>

#include "mbedtls/x509_crt.h"
#include "mbedtls/x509_csr.h"

#include "check.h"
#include "fake_store.h"
#include "memory_identity.h"
#include "eink/tls/flash_identity.h"
#include "eink/tls/est_client.h"
#include "eink/core/join.h"
#include "eink/tls/stream.h"
#include "eink/tls/verifier.h"

namespace support {

using eink_join::Joiner;
using eink_join::Outcome;
using eink_pairing::Standing;
using eink_report::Failure;
using host::MemoryIdentity;


constexpr int64_t DAY = 86400;

inline std::string environment(const char *name) {
  const char *value = std::getenv(name);
  if (value == nullptr) {
    std::fprintf(stderr, "%s is not set: run through host/with_fixture.sh\n", name);
    std::exit(2);
  }
  return value;
}

inline uint16_t server_port() { return static_cast<uint16_t>(std::atoi(environment("FIXTURE_PORT").c_str())); }
inline uint32_t server_ip() { return inet_addr("127.0.0.1"); }  // network order: the first octet in the lowest byte, as lwIP

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

// A display that keeps what it holds in "flash" and starts afresh at every wake, as the chip does after deep sleep:
// nothing lives in memory between wakes but the flash and the one value RTC memory keeps. This is what shows the record
// is enough.
struct Rebooting {
  std::string name;
  fakes::MemoryStore flash;
  eink_pairing::Answer rtc = eink_pairing::Answer::NONE;  // survives sleep, not a power loss
  HostClock clock;
  int roots = 0, enrolled = 0, renewed = 0;
  eink_ports::Bytes compiled;

  explicit Rebooting(const std::string &n) : name(n) {}

  Outcome wake() {
    flash.restore_power();
    eink_flash::FlashIdentity identity(flash, rtc, compiled);
    CountingEst est(server_ip(), server_port(), identity);
    eink_verifier::MbedVerifier verifier;
    const Outcome out = Joiner(clock, identity, est, verifier, name).run();
    roots += est.roots;
    enrolled += est.enrolled;
    renewed += est.renewed;
    return out;
  }
  // What is held, as a fresh start reads it.
  eink_credentials::Record held() {
    eink_credentials::Credentials c(flash);
    return c.record();
  }
};

inline int ctl(const std::string &arguments) {
  const std::string command =
      environment("DISPLAYCTL") + " --socket " + environment("FIXTURE_ADMIN") + " " + arguments + " > /dev/null 2>&1";
  return std::system(command.c_str());
}

// A certificate that names itself and nobody else vouches for: someone in the middle's root.
inline eink_ports::Bytes someone_elses_root() {
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
inline void join(Display &d) {
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK_EQ(ctl("approve " + d.name + " " + waiting.code), 0);
  const Outcome paired = d.wake();
  CHECK(paired.paired);
}

// What a stream needs to speak to the server as a display that has joined.
inline eink_stream::Peer peer_of(Display &d) {
  eink_stream::Peer peer;
  peer.ip = server_ip();
  peer.port = server_port();
  peer.root = d.identity.stored;
  peer.certificate = d.identity.certificate_der;
  peer.key = &d.identity.private_key();
  return peer;
}

// A display that has joined: waits, is approved by its code, and is paired.
inline void join_as(Display &d) { join(d); }

}  // namespace support
