// What the host tests share: where the real server is (from the environment with_fixture.sh sets), the computer's clock, the
// real EST client counted, a display held in memory, the admin commands, and taking a display from nothing to paired.
#pragma once

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>
#include <cstdlib>
#include <ctime>
#include <string>

#include "mbedtls/x509_crt.h"
#include "mbedtls/x509_csr.h"

#include "check.h"
#include "fake_store.h"
#include "memory_identity.h"
#include "home_display/tls/flash_identity.h"
#include "home_display/tls/est_client.h"
#include "home_display/core/join.h"
#include "home_display/tls/stream.h"
#include "home_display/tls/verifier.h"

namespace support {

using home_display_join::Joiner;
using home_display_join::Outcome;
using home_display_pairing::Standing;
using home_display_report::Failure;
using host::MemoryIdentity;


constexpr int64_t DAY = 86400;

inline std::string environment(const char *name) {
  const char *value = std::getenv(name);
  if (value == nullptr) {
    std::fprintf(stderr, "%s is not set: run through tests/tls/with_fixture.sh\n", name);
    std::exit(2);
  }
  return value;
}

inline uint16_t server_port() { return static_cast<uint16_t>(std::atoi(environment("FIXTURE_PORT").c_str())); }
inline uint32_t server_ip() { return inet_addr("127.0.0.1"); }  // network order: the first octet in the lowest byte, as lwIP

// The computer's clock, which a test can move forward to be at a certificate's third, or past its end.
struct HostClock : home_display_ports::Clock {
  int64_t offset = 0;
  int64_t now() override { return static_cast<int64_t>(std::time(nullptr)) + offset; }
  bool usable() override { return true; }
};

// The real client, counted.
struct CountingEst : home_display_ports::Est {
  home_display_est::EstClient inner;
  int roots = 0, enrolled = 0, renewed = 0;
  CountingEst(uint32_t ip, uint16_t port, home_display_tls::TlsIdentity &identity) : inner(ip, port, identity) {}
  home_display_ports::RootReply fetch_root() override {
    roots++;
    return inner.fetch_root();
  }
  home_display_ports::Reply enroll(const std::string &n, home_display_ports::Identity &i, const home_display_ports::Bytes &r) override {
    enrolled++;
    return inner.enroll(n, i, r);
  }
  home_display_ports::Reply renew(const std::string &n, home_display_ports::Identity &i, const home_display_ports::Bytes &r) override {
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
  home_display_verifier::MbedVerifier verifier;

  explicit Display(const std::string &n, uint16_t port = server_port()) : name(n), est(server_ip(), port, identity) {}
  Outcome wake() { return Joiner(clock, identity, est, verifier, name).run(); }
};

// A display that keeps what it holds in "flash" and starts afresh at every wake, as the chip does after deep sleep:
// nothing lives in memory between wakes but the flash and the one value RTC memory keeps. This is what shows the record
// is enough.
struct Rebooting {
  std::string name;
  fakes::MemoryStore flash;
  home_display_pairing::Answer rtc = home_display_pairing::Answer::NONE;  // survives sleep, not a power loss
  HostClock clock;
  int roots = 0, enrolled = 0, renewed = 0;
  home_display_ports::Bytes compiled;

  explicit Rebooting(const std::string &n) : name(n) {}

  Outcome wake() {
    flash.restore_power();
    home_display_flash::FlashIdentity identity(flash, rtc, compiled);
    CountingEst est(server_ip(), server_port(), identity);
    home_display_verifier::MbedVerifier verifier;
    const Outcome out = Joiner(clock, identity, est, verifier, name).run();
    roots += est.roots;
    enrolled += est.enrolled;
    renewed += est.renewed;
    return out;
  }
  // What is held, as a fresh start reads it.
  home_display_credentials::Record held() {
    home_display_credentials::Credentials c(flash);
    return c.record();
  }
};

inline int ctl(const std::string &arguments) {
  const std::string command =
      environment("DISPLAYCTL") + " --socket " + environment("FIXTURE_ADMIN") + " " + arguments + " > /dev/null 2>&1";
  return std::system(command.c_str());
}

// A certificate that names itself and nobody else vouches for: someone in the middle's root.
inline home_display_ports::Bytes someone_elses_root() {
  mbedtls_pk_context key;
  mbedtls_pk_init(&key);
  mbedtls_pk_setup(&key, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY));
  mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(key), home_display_tls::Random::generate, nullptr);
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
  const int n = mbedtls_x509write_crt_der(&crt, buffer, sizeof buffer, home_display_tls::Random::generate, nullptr);
  mbedtls_x509write_crt_free(&crt);
  mbedtls_pk_free(&key);
  return n > 0 ? home_display_ports::Bytes(buffer + sizeof buffer - n, buffer + sizeof buffer) : home_display_ports::Bytes();
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
// A listening socket that takes connections and never answers: a server that is there and says nothing.
class QuietServer {
 public:
  QuietServer() {
    fd_ = ::socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in address = {};
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = server_ip();
    CHECK(::bind(fd_, reinterpret_cast<struct sockaddr *>(&address), sizeof address) == 0);
    CHECK(::listen(fd_, 4) == 0);
    socklen_t size = sizeof address;
    ::getsockname(fd_, reinterpret_cast<struct sockaddr *>(&address), &size);
    port_ = ntohs(address.sin_port);
  }
  ~QuietServer() { ::close(fd_); }
  QuietServer(const QuietServer &) = delete;
  QuietServer &operator=(const QuietServer &) = delete;
  uint16_t port() const { return port_; }

 private:
  int fd_ = -1;
  uint16_t port_ = 0;
};

inline home_display_stream::Peer peer_of(Display &d) {
  home_display_stream::Peer peer;
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
