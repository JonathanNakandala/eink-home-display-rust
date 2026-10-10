// How a wake begins its dealings with the server when it may speak TLS: join if need be, decide the route
// (core/secure_wake.h), and when the display has joined, record the server for the requests that follow
// (tls/secure.h).
//
// This is what the wake script calls first, with the chip's clock and flash (esp/secure_chip.h) or a computer's
// stand-ins, so that the whole of it, joining, deciding and the first request, runs against a real server on a computer
// (tests/tls/).
#pragma once

#include <string>

#include "home_display/tls/est_client.h"
#include "home_display/core/join.h"
#include "home_display/core/pairing.h"
#include "home_display/core/ports.h"
#include "home_display/tls/secure.h"
#include "home_display/core/session_cache.h"
#include "home_display/core/secure_wake.h"
#include "home_display/core/service.h"
#include "home_display/tls/tls.h"
#include "home_display/tls/verifier.h"

namespace home_display_secure_begin {

// What the wake script is told about how to reach the server.
struct Begin {
  home_display_secure_wake::Verdict verdict;
  std::string prompt;  // what the panel shows for the owner, with the code, when `verdict.prompt`
  uint32_t retry_after_s = 0;
};

// `ip` and `tls_port` are what discovery found (0 for neither). If the display has joined the server is recorded in
// tls/secure.h, and the display's root, certificate and key (kept in `identity`) are what it will show.
//
// `sessions`, if given, is where the TLS session of this server and this display is kept, so that the plan and the
// picture of the wake, and the wakes after, resume it (core/session_cache.h). It is asked for the store of this server
// and this display once the display has joined.
inline Begin begin(home_display_service::Transport transport, uint32_t ip, uint16_t tls_port, const std::string &name,
                   home_display_ports::Clock &clock, home_display_tls::TlsIdentity &identity,
                   home_display_session_cache::Holder *sessions = nullptr) {
  Begin out;
  home_display_join::Outcome joined;
  const bool addressable = ip != 0 && tls_port != 0;
  home_display_secure::forget();
  if (transport != home_display_service::Transport::HTTP && addressable) {
    home_display_est::EstClient est(ip, tls_port, identity);
    home_display_verifier::MbedVerifier verifier;
    joined = home_display_join::Joiner(clock, identity, est, verifier, name).run();
    if (joined.paired) {
      const home_display_ports::Bytes compiled = identity.compiled_root();
      const home_display_ports::Bytes root = compiled.empty() ? identity.stored_root() : compiled;
      const home_display_ports::Bytes certificate = identity.held_certificate();
      home_display_secure::use(ip, tls_port, root, certificate, &identity.private_key(), home_display_tls::WAIT_MS,
                               sessions == nullptr ? nullptr
                                                   : sessions->select(home_display_session_cache::make_key(
                                                         ip, tls_port, root, certificate)));
    }
  }
  out.verdict = home_display_secure_wake::decide(transport, addressable, joined);
  out.retry_after_s = joined.retry_after_s;
  if (out.verdict.prompt)
    out.prompt = home_display_pairing::notice(joined.standing, joined.code);
  return out;
}

}  // namespace home_display_secure_begin
