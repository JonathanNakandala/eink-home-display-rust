// What a wake does with the secure transport: whether to speak TLS, fall back to plain HTTP, wait for the owner, or
// give up, from how the display is set (core/service.h) and how the join went (core/join.h).
//
// This is the table of the three transports in the README, in code:
//   http          never joins, never speaks TLS.
//   prefer-https  speaks TLS when it has joined, and on any failure goes on over plain HTTP: someone who can break the
//                 TLS can also strip it, so refusing would add no protection, only a display that stops. It is for
//                 moving over. It still shows the pairing code while the owner has to approve.
//   https         TLS only, never plain HTTP. If it cannot, the wake fails like any other; when it is only waiting for
//                 the owner, it waits without counting a failure.
#pragma once

#include <cstdint>

#include "home_display/core/join.h"
#include "home_display/core/pairing.h"
#include "home_display/core/report.h"
#include "home_display/core/service.h"
#include "home_display/core/link.h"
#include "home_display/core/wake.h"

namespace home_display_secure_wake {

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
  home_display_report::Failure failure = home_display_report::Failure::NONE;
  // The owner has to type the code in at the server, so the panel must show it.
  bool prompt = false;
};

inline bool needs_owner(home_display_pairing::Standing standing) {
  return standing == home_display_pairing::Standing::WAITING ||
         standing == home_display_pairing::Standing::NOT_RECOGNISED;
}

// `addressable`: the server was found and announces an HTTPS port, which joining needs. `join` is how joining went
// (ignored when it was not addressable).
inline Verdict decide(home_display_service::Transport transport, bool addressable,
                      const home_display_join::Outcome &join) {
  using home_display_report::Failure;
  using home_display_service::Transport;
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

// Whether this wake leaves HTTPS alone, without looking for the server's HTTPS port, waiting for the clock or telling
// anyone it failed: a display that prefers HTTPS, while it remembers that the server it found has none
// (core/no_https.h).
inline bool leaves_https_alone(home_display_service::Transport transport, bool remembered_no_https) {
  return transport == home_display_service::Transport::PREFER_HTTPS && remembered_no_https;
}

// What a search for the server changes in what is remembered.
enum class Memo : uint8_t {
  KEEP,      // as it was
  REMEMBER,  // the server was found, and has no HTTPS
  FORGET,    // the server was found, and has HTTPS
};

// Only a display that prefers HTTPS remembers, and only from a search that found the server: one that found nothing
// says nothing about whether it serves HTTPS, and a display set to `https` is waiting for the server to offer it and
// must go on looking.
inline Memo after_search(home_display_service::Transport transport, bool found, uint16_t tls_port) {
  if (!found || transport != home_display_service::Transport::PREFER_HTTPS)
    return Memo::KEEP;
  return tls_port == 0 ? Memo::REMEMBER : Memo::FORGET;
}

// A request over TLS failed while the display was paired. `last` is how it got on
// (home_display_secure::context().last).
inline home_display_report::Failure failure_after(home_display_link::Start last) {
  return last == home_display_link::Start::REFUSED ? home_display_report::Failure::CERTIFICATE
                                                   : home_display_report::Failure::SERVER;
}

// Whether to try again over plain HTTP: only a display that prefers HTTPS does. One set to https never does.
inline bool falls_back(home_display_service::Transport transport) {
  return transport == home_display_service::Transport::PREFER_HTTPS;
}

// Why a download failed: a certificate turned down is its own reason, anything else is the download.
inline home_display_report::Failure download_failure(bool over_tls, home_display_link::Start last) {
  return over_tls && last == home_display_link::Start::REFUSED ? home_display_report::Failure::CERTIFICATE
                                                               : home_display_report::Failure::DOWNLOAD;
}

// How long to sleep before asking again while waiting for the owner: what the server said (Retry-After), kept between a
// minute and a day like any other sleep; five minutes if it said nothing.
inline uint32_t wait_sleep_ms(uint32_t retry_after_s) {
  return home_display_wake::plan_sleep_ms(retry_after_s ? retry_after_s : 300);
}

}  // namespace home_display_secure_wake
