// Joining the server's private certificate authority: the code the owner types in, and what the display does next.
//
// The server computes the same code (PairingCode::derive in src/domain/models/pairing.rs), and the two are pinned to
// the same example, so a difference in either shows up as a failing test.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include "home_display/core/report.h"
#include "home_display/core/sha256.h"
#include "home_display/core/trust.h"

namespace home_display_pairing {

// ---- the code -----------------------------------------------------------------------------------------------------

// Twelve characters of Crockford's Base32 (5 bits each, 60 bits). Short enough to type, long enough that someone in the
// middle, who can choose what the two sides see, cannot search for a pair that agrees.
constexpr size_t CODE_CHARACTERS = 12;
constexpr size_t CODE_GROUP = 4;
// Digits and letters without I, L, O and U, so nothing read off a small panel is taken for something else.
constexpr char ALPHABET[] = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

// What the display works out and shows, from what *it* saw (never from anything the server says):
// SHA-256("home-display pairing code v1" || SHA-256(root DER) || u64be(len(name)) || name || u64be(len(SPKI)) ||
// SPKI), the first 60 bits as twelve 5-bit values, most significant first, as `XXXX-XXXX-XXXX`. `spki` is the DER
// SubjectPublicKeyInfo of the display's key.
inline std::string code(const std::vector<uint8_t> &root_der, const std::string &name,
                        const std::vector<uint8_t> &spki) {
  const home_display_sha256::Digest root = home_display_sha256::of(root_der);
  home_display_sha256::Hash hash;
  hash.update("home-display pairing code v1");
  hash.update(root);
  hash.update_length(name.size());
  hash.update(name);
  hash.update_length(spki.size());
  hash.update(spki);
  const home_display_sha256::Digest digest = hash.finish();

  uint64_t first = 0;
  for (int i = 0; i < 8; i++)
    first = (first << 8) | digest[i];
  const uint64_t number = first >> (64 - 5 * CODE_CHARACTERS);

  std::string out;
  for (size_t i = 0; i < CODE_CHARACTERS; i++) {
    if (i != 0 && i % CODE_GROUP == 0)
      out += '-';
    out += ALPHABET[(number >> (5 * (CODE_CHARACTERS - 1 - i))) & 31];
  }
  return out;
}

// ---- what to do next ----------------------------------------------------------------------------------------------

// What the display holds, as far as deciding goes.
struct Stored {
  home_display_trust::RootSource root;
  bool has_key;
  bool has_certificate;
  int64_t not_before;  // of the certificate, if there is one
  int64_t not_after;
};

// What the server last said to this display's request at this step, if it said anything.
enum class Answer : uint8_t {
  NONE,     // nothing yet, or the server could not be reached (that is a different failure, retried as it was)
  PENDING,  // 202: not approved yet, ask again after Retry-After
  REFUSED,  // 403: the owner declined or the window is not open, or this key is not one the server knows
};

// The state the owner can see on the panel, each with its own meaning.
enum class Standing : uint8_t {
  NO_ROOT,  // fetching the root
  ASKING,   // has a key and a root, no certificate yet
  WAITING,  // asked, and the owner has not approved: the code is on the panel
  PAIRED,
  RENEWING,
  EXPIRED,         // the certificate ended; asking again with the same key, which needs no owner
  NOT_RECOGNISED,  // the server does not know this key: the owner must approve it, and the code is on the panel
  CLOCK_NOT_SET,   // nothing that needs the time can be tried
};

// The one thing to do this wake.
enum class Step : uint8_t {
  NOTHING,     // wait for the clock
  FETCH_ROOT,  // GET cacerts, unverified, and keep only the self-signed root
  MAKE_KEY,    // make the ECDSA P-256 key, once
  ENROLL,      // POST simpleenroll, not showing a certificate (there is none, or it is no use)
  RENEW,       // POST simplereenroll, showing the current certificate, with the same key
  USE,         // paired: carry on with the wake
};

struct Next {
  Standing standing;
  Step action;
};

// Decides from what is stored, the clock and the server's last answer. `clock_usable` is home_display_clock::usable.
// A certificate from the future means the clock is behind, not that the certificate is bad.
inline Next next(const Stored &stored, int64_t now, bool clock_usable, Answer last) {
  if (!clock_usable)
    return {Standing::CLOCK_NOT_SET, Step::NOTHING};
  if (home_display_trust::needs_root_fetched(stored.root))
    return {Standing::NO_ROOT, Step::FETCH_ROOT};
  if (!stored.has_key)
    return {Standing::ASKING, Step::MAKE_KEY};

  // Asking without a certificate to show: the first time, after it ended, or after the server stopped knowing it.
  const auto ask = [&](Standing otherwise) -> Next {
    if (last == Answer::PENDING)
      return {Standing::WAITING, Step::ENROLL};
    if (last == Answer::REFUSED)
      return {Standing::NOT_RECOGNISED, Step::ENROLL};
    return {otherwise, Step::ENROLL};
  };

  if (!stored.has_certificate)
    return ask(Standing::ASKING);
  switch (home_display_trust::standing(now, stored.not_before, stored.not_after)) {
    case home_display_trust::Standing::NOT_YET: return {Standing::CLOCK_NOT_SET, Step::NOTHING};
    case home_display_trust::Standing::EXPIRED: return ask(Standing::EXPIRED);
    case home_display_trust::Standing::DUE:
      // A renewal the server refuses means it no longer knows this display (revoked or forgotten): ask as a new one.
      return last == Answer::REFUSED ? ask(Standing::NOT_RECOGNISED) : Next{Standing::RENEWING, Step::RENEW};
    case home_display_trust::Standing::VALID:
      return last == Answer::REFUSED ? ask(Standing::NOT_RECOGNISED) : Next{Standing::PAIRED, Step::USE};
  }
  return {Standing::PAIRED, Step::USE};
}

// What to report to the server, as the next wake that gets through. A paired or renewing display reports nothing: its
// wake succeeds or fails like any other.
inline home_display_report::Failure failure_for(Standing standing) {
  switch (standing) {
    case Standing::CLOCK_NOT_SET: return home_display_report::Failure::CLOCK;
    case Standing::WAITING: return home_display_report::Failure::APPROVAL;
    case Standing::NOT_RECOGNISED: return home_display_report::Failure::UNRECOGNISED;
    default: return home_display_report::Failure::NONE;
  }
}

// What the panel says, with the code the owner is to type in where there is one.
inline std::string notice(Standing standing, const std::string &code) {
  switch (standing) {
    case Standing::NO_ROOT: return "Finding the server";
    case Standing::ASKING: return "Asking to join";
    case Standing::WAITING: return "Waiting for approval: " + code;
    case Standing::EXPIRED: return "Joining again";
    case Standing::NOT_RECOGNISED: return "Not recognised - ask the owner to approve: " + code;
    case Standing::CLOCK_NOT_SET: return "Clock not set";
    default: return "";  // paired, renewing: nothing to say
  }
}

}  // namespace home_display_pairing
