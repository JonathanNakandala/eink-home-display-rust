// How a wake begins its dealings with the server when it may speak TLS: join if need be, decide the route
// (eink_secure_wake.h), and when the display has joined, record the server for the requests that follow
// (eink_secure.h).
//
// This is what the wake script calls first, with the chip's clock and flash (eink_secure_chip.h) or a computer's
// stand-ins, so that the whole of it, joining, deciding and the first request, runs against a real server on a computer
// (host/).
#pragma once

#include <string>

#include "eink_est_client.h"
#include "eink_join.h"
#include "eink_pairing.h"
#include "eink_ports.h"
#include "eink_secure.h"
#include "eink_secure_wake.h"
#include "eink_service.h"
#include "eink_tls.h"
#include "eink_verifier.h"

namespace eink_secure_begin {

// What the wake script is told about how to reach the server.
struct Begin {
  eink_secure_wake::Verdict verdict;
  std::string prompt;  // what the panel shows for the owner, with the code, when `verdict.prompt`
  uint32_t retry_after_s = 0;
};

// `ip` and `tls_port` are what discovery found (0 for neither). If the display has joined the server is recorded in
// eink_secure.h, and the display's root, certificate and key (kept in `identity`) are what it will show.
inline Begin begin(eink_service::Transport transport, uint32_t ip, uint16_t tls_port, const std::string &name,
                   eink_ports::Clock &clock, eink_tls::TlsIdentity &identity) {
  Begin out;
  eink_join::Outcome joined;
  const bool addressable = ip != 0 && tls_port != 0;
  eink_secure::forget();
  if (transport != eink_service::Transport::HTTP && addressable) {
    eink_est::EstClient est(ip, tls_port, identity);
    eink_verifier::MbedVerifier verifier;
    joined = eink_join::Joiner(clock, identity, est, verifier, name).run();
    if (joined.paired) {
      const eink_ports::Bytes compiled = identity.compiled_root();
      eink_secure::use(ip, tls_port, compiled.empty() ? identity.stored_root() : compiled, identity.held_certificate(),
                       &identity.private_key());
    }
  }
  out.verdict = eink_secure_wake::decide(transport, addressable, joined);
  out.retry_after_s = joined.retry_after_s;
  if (out.verdict.prompt)
    out.prompt = eink_pairing::notice(joined.standing, joined.code);
  return out;
}

}  // namespace eink_secure_begin
