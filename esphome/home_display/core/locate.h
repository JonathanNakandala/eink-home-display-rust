// Where the server is at the start of a wake: the address remembered from the last one, if something still answers
// there, else a search of the network.
//
// The address is only a cache of what discovery (esp/discovery.h) found, kept so that a normal wake does no lookup. It
// is never trusted: nothing here, or anywhere, may depend on a network handing out the same address twice, a router
// having a reservation, or a server staying on a port. A server that moves is found again on the very next wake, in a
// few seconds.
//
// What used to happen instead was that the dead address was used, and the search came only after its request had timed
// out: fifteen seconds over TLS, fifty over plain HTTP, and for a display that prefers HTTPS both, since it falls back
// to plain HTTP before it looks. A display set to HTTPS that had to join or renew went straight to a failure without
// looking at all. Checking first costs a TCP connection to a server that is there, and saves a minute when it is not.
//
// Decided here, tested on a computer with the network replaced by two functions; the chip's are esp/reach.h (does
// something accept a connection there) and esp/discovery.h (the mDNS query).
#pragma once

#include <cstdint>

#include "home_display/core/service.h"

namespace home_display_locate {

// A single multicast query can be lost (Wi-Fi sends multicast at the lowest rate with no retry), and a lost query used
// to be a failed wake, which backs off. So a search asks this many times, and stops at the first answer.
constexpr int SEARCH_ATTEMPTS = 3;

// How the wake stands with the server after looking.
enum class Where : uint8_t {
  REMEMBERED,  // the address from last time answers: nothing has changed, and nothing was searched
  FOUND,       // a search found the server (`found` says where; it may be the address it had)
  UNKNOWN,     // there was no address to check, and the search found nothing: go on by the server's name
  GONE,        // there was an address, nothing answers there, and the search found nothing
};

// Asks `once` (one search of the network, filling `found`) up to SEARCH_ATTEMPTS times.
template <class Once> bool search(Once &&once, home_display_service::Server &found) {
  for (int attempt = 0; attempt < SEARCH_ATTEMPTS; attempt++) {
    if (once(found))
      return true;
  }
  return false;
}

// The port this wake will talk to first: the HTTPS one when HTTPS is in play and the server has one, else the HTTP one.
inline uint16_t port_to_check(bool https_in_play, uint16_t port, uint16_t tls_port) {
  return https_in_play && tls_port != 0 ? tls_port : port;
}

// `https_in_play`: the wake may speak TLS (the transport is not `http`, and a display that prefers HTTPS is not leaving
// it alone because it remembers the server has none). `ip`, `port` and `tls_port` are what is remembered (0 for none).
// `reaches(ip, port)`: whether something accepts a connection there. `once(found)`: one search.
//
// A display that is to speak TLS and has no HTTPS port remembered has nothing to check, and searches: the server may
// have begun to offer it.
//
// When the remembered address is silent and the search finds nothing, the address is checked once more before it is
// given up on: the multicast may have been lost three times, and a server that was only slow to accept is still the
// server.
template <class Reaches, class Once> Where locate(bool https_in_play, uint32_t ip, uint16_t port, uint16_t tls_port,
                                                  Reaches &&reaches, Once &&once, home_display_service::Server &found) {
  const uint16_t check = port_to_check(https_in_play, port, tls_port);
  const bool knew = ip != 0 && check != 0 && !(https_in_play && tls_port == 0);
  if (knew && reaches(ip, check))
    return Where::REMEMBERED;
  if (search(once, found))
    return Where::FOUND;
  if (!knew)
    return Where::UNKNOWN;
  if (reaches(ip, check))
    return Where::REMEMBERED;
  return Where::GONE;
}

}  // namespace home_display_locate
