// Which root the display trusts, and where a certificate stands in its life.
//
// Parsing a certificate and verifying a chain are mbedTLS's job; this decides from the answers.
#pragma once

#include <cstdint>

namespace home_display_trust {

// Where the root the display trusts comes from.
//  - COMPILED: the owner's `root.pem`, built into the firmware (`server_root` in secrets.yaml). It wins over
//    anything else and nothing is ever fetched: a server that shows another root is refused, not believed.
//  - STORED: fetched once from the server and confirmed by the pairing code, kept in flash.
//  - NONE: neither yet, so the root has to be fetched (an unverified connection, confirmed later by the code).
enum class RootSource : uint8_t { NONE, STORED, COMPILED };

inline RootSource root_source(bool compiled_in, bool stored) {
  return compiled_in ? RootSource::COMPILED : stored ? RootSource::STORED : RootSource::NONE;
}

// Only a display with no root at all fetches one. With one compiled in or stored it is never replaced by what the
// server says, which is how someone in the middle would hand over their own.
inline bool needs_root_fetched(RootSource source) { return source == RootSource::NONE; }

// Where a certificate is in its life, by the clock now.
enum class Standing : uint8_t {
  NOT_YET,  // the clock reads before the certificate starts: the clock is wrong, not the certificate
  VALID,
  DUE,  // a third of its life or less is left: renew
  EXPIRED,
};

// A certificate from `not_before` to `not_after` (seconds since 1970). It is renewed with a third of its life left (the
// server issues 90 days, so at 30), checked at every wake and retried at the next if it fails. The end is exclusive:
// at `not_after` it is over.
inline Standing standing(int64_t now, int64_t not_before, int64_t not_after) {
  if (now < not_before)
    return Standing::NOT_YET;
  if (now >= not_after)
    return Standing::EXPIRED;
  const int64_t life = not_after - not_before;
  return (not_after - now) * 3 <= life ? Standing::DUE : Standing::VALID;
}

}  // namespace home_display_trust
