// How a wake begins its dealings with the server when it may speak TLS: join if need be, decide the route
// (core/secure_wake.h), and when the display has joined, record the server for the requests that follow
// (tls/secure.h).
//
// This is what the wake script calls first, with the chip's clock and flash (esp/secure_chip.h) or a computer's
// stand-ins, so that the whole of it, joining, deciding and the first request, runs against a real server on a computer
// (host/).
#pragma once

#include <string>

#include "eink/tls/est_client.h"
#include "eink/core/join.h"
#include "eink/core/pairing.h"
#include "eink/core/ports.h"
#include "eink/tls/secure.h"
#include "eink/core/session_cache.h"
#include "eink/core/secure_wake.h"
#include "eink/core/service.h"
#include "eink/tls/tls.h"
#include "eink/tls/verifier.h"

namespace eink_secure_begin {

// What the wake script is told about how to reach the server.
struct Begin {
  eink_secure_wake::Verdict verdict;
  std::string prompt;  // what the panel shows for the owner, with the code, when `verdict.prompt`
  uint32_t retry_after_s = 0;
};

// `ip` and `tls_port` are what discovery found (0 for neither). If the display has joined the server is recorded in
// tls/secure.h, and the display's root, certificate and key (kept in `identity`) are what it will show.
//
// `sessions`, if given, is where the TLS session of this server and this display is kept, so that the plan and the
// picture of the wake, and the wakes after, resume it (core/session_cache.h). It is asked for the store of this server
// and this display once the display has joined.
inline Begin begin(eink_service::Transport transport, uint32_t ip, uint16_t tls_port, const std::string &name,
                   eink_ports::Clock &clock, eink_tls::TlsIdentity &identity,
                   eink_session_cache::Holder *sessions = nullptr) {
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
      const eink_ports::Bytes root = compiled.empty() ? identity.stored_root() : compiled;
      const eink_ports::Bytes certificate = identity.held_certificate();
      eink_secure::use(ip, tls_port, root, certificate, &identity.private_key(), 15000,
                       sessions == nullptr
                           ? nullptr
                           : sessions->select(eink_session_cache::make_key(ip, tls_port, root, certificate)));
    }
  }
  out.verdict = eink_secure_wake::decide(transport, addressable, joined);
  out.retry_after_s = joined.retry_after_s;
  if (out.verdict.prompt)
    out.prompt = eink_pairing::notice(joined.standing, joined.code);
  return out;
}

}  // namespace eink_secure_begin
