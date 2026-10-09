// What a wake does with the secure transport: whether to speak TLS, fall back to plain HTTP, wait for the owner, or
// give up, from how the display is set (eink_service.h) and how the join went (eink_join.h).
//
// Pure calculation, with nothing from ESPHome or ESP-IDF, so it is compiled and tested on a computer (tests/). The
// README's table of the three transports is this, in code:
//   http          never joins, never speaks TLS.
//   prefer-https  speaks TLS when it has joined and falls back to plain HTTP on any failure, because someone who can
//   break
//                 the TLS can also strip it, so refusing to fall back adds no protection, only a display that stops. It
//                 is for moving over. It still shows the pairing code while the owner has to approve.
//   https         TLS only, never plain HTTP. If it cannot, the wake fails like any other (or, when it is only waiting
//   for the
//                 owner, waits without counting a failure).
#pragma once

#include <cstdint>

#include "eink_join.h"
#include "eink_pairing.h"
#include "eink_report.h"
#include "eink_service.h"
#include "eink_link.h"
#include "eink_wake.h"

namespace eink_secure_wake {

// What to do about reaching the server this wake.
enum class Route : uint8_t {
  SECURE,          // speak TLS
  PLAIN,           // speak plain HTTP (the transport is http, or it fell back)
  WAIT_FOR_OWNER,  // https only: nothing can be done until the owner approves; sleep and ask again
  FAIL,            // https only: the wake fails with `failure`
};

struct Verdict {
  Route route = Route::PLAIN;
  // Why it is not simply secure, for the server to be told (and the log): none when it is.
  eink_report::Failure failure = eink_report::Failure::NONE;
  // The owner has to type the code in at the server, so the panel must show it.
  bool prompt = false;
};

inline bool needs_owner(eink_pairing::Standing standing) {
  return standing == eink_pairing::Standing::WAITING || standing == eink_pairing::Standing::NOT_RECOGNISED;
}

// `addressable`: the server was found and announces an HTTPS port, which joining needs. `join` is how joining went
// (ignored when it was not addressable).
inline Verdict decide(eink_service::Transport transport, bool addressable, const eink_join::Outcome &join) {
  using eink_report::Failure;
  using eink_service::Transport;
  if (transport == Transport::HTTP)
    return {Route::PLAIN, Failure::NONE, false};
  if (addressable && join.paired)
    return {Route::SECURE, Failure::NONE, false};
  const bool owner = addressable && needs_owner(join.standing);
  // No HTTPS port to join at: the server is not serving HTTPS, or not found.
  const Failure failure = addressable ? join.failure : Failure::SERVER;
  if (transport == Transport::HTTPS)
    return owner ? Verdict{Route::WAIT_FOR_OWNER, failure, true} : Verdict{Route::FAIL, failure, false};
  return {Route::PLAIN, failure, owner};
}

// A request over TLS failed while the display was paired. `last` is how it got on (eink_secure::context().last).
inline eink_report::Failure failure_after(eink_link::Start last) {
  return last == eink_link::Start::REFUSED ? eink_report::Failure::CERTIFICATE : eink_report::Failure::SERVER;
}

// Whether to try again over plain HTTP: only a display that prefers HTTPS does. One set to https never does.
inline bool falls_back(eink_service::Transport transport) { return transport == eink_service::Transport::PREFER_HTTPS; }

// Why a download failed: a certificate turned down is its own reason, anything else is the download.
inline eink_report::Failure download_failure(bool over_tls, eink_link::Start last) {
  return over_tls && last == eink_link::Start::REFUSED ? eink_report::Failure::CERTIFICATE
                                                       : eink_report::Failure::DOWNLOAD;
}

// How long to sleep before asking again while waiting for the owner: what the server said (Retry-After), kept between a
// minute and a day like any other sleep; five minutes if it said nothing.
inline uint32_t wait_sleep_ms(uint32_t retry_after_s) {
  return eink_wake::plan_sleep_ms(retry_after_s ? retry_after_s : 300);
}

}  // namespace eink_secure_wake
