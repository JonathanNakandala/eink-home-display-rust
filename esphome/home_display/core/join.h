// One wake's worth of joining the server's authority: decide what to do (core/pairing.h), do it through the interfaces
// (core/ports.h), and say where it got to. Pure logic over interfaces, so it is tested on a computer with fakes; the
// chip supplies the real ones.
//
// The root is the root fetched from an unverified connection, so until a certificate has been issued under it the
// display does not keep it: it is fetched again each wake. The code on the panel is worked out from the root the
// display saw this wake, and the owner approves only if it matches the one the server worked out from its own root, so
// someone in the middle who shows another root is caught. Only a certificate that chains to the root, issued after the
// owner approved, makes the display keep it; from then on it is the one it trusts.
#pragma once

#include <cstdint>
#include <string>

#include "home_display/core/pairing.h"
#include "home_display/core/ports.h"
#include "home_display/core/report.h"
#include "home_display/core/trust.h"

namespace home_display_join {

struct Outcome {
  home_display_pairing::Standing standing = home_display_pairing::Standing::NO_ROOT;
  // Why it stopped short, for the report to the server: the standing's own (clock, approval, not recognised), or one
  // of the failures only a request can have (server unreachable, a certificate refused, nothing could be written).
  home_display_report::Failure failure = home_display_report::Failure::NONE;
  bool paired = false;         // the display holds a good certificate: carry on with the wake over TLS
  uint32_t retry_after_s = 0;  // how long to sleep before asking again, when the server said
  std::string code;            // what the panel shows, when it shows one
};

class Joiner {
 public:
  Joiner(home_display_ports::Clock &clock, home_display_ports::Identity &identity, home_display_ports::Est &est,
         home_display_ports::Verifier &verifier, std::string name)
      : clock_(clock), identity_(identity), est_(est), verifier_(verifier), name_(std::move(name)) {}

  Outcome run() {
    // Named one by one: a `using namespace` here would make names like Step ambiguous with ESPHome's own, which the
    // generated main.cpp has in scope.
    using home_display_pairing::Answer;
    using home_display_pairing::code;
    using home_display_pairing::failure_for;
    using home_display_pairing::Next;
    using home_display_pairing::next;
    using home_display_pairing::Standing;
    using home_display_pairing::Step;
    using home_display_pairing::Stored;
    Outcome out;
    home_display_ports::Bytes provisional;  // fetched this wake and not yet confirmed by a certificate
    if (!identity_.readable()) {
      // What is kept cannot be read, so it is not touched and nothing is tried: not even asking the server.
      out.failure = home_display_report::Failure::MEMORY;
      return out;
    }
    // Each step settles one thing, so a few is enough; the bound is only against a loop that cannot end.
    for (int step = 0; step < 8; step++) {
      const home_display_ports::Bytes compiled = identity_.compiled_root();
      const home_display_ports::Bytes stored = identity_.stored_root();
      const bool have_certificate = identity_.has_certificate();
      const home_display_ports::Lifetime life = identity_.lifetime();
      const Stored view{home_display_trust::root_source(!compiled.empty(), !stored.empty() || !provisional.empty()),
                        identity_.has_key(), have_certificate, life.not_before, life.not_after};
      const Next n = next(view, clock_.now(), clock_.usable(), identity_.last_answer());
      out.standing = n.standing;
      out.failure = failure_for(n.standing);
      const home_display_ports::Bytes &root = !compiled.empty() ? compiled : !stored.empty() ? stored : provisional;

      switch (n.action) {
        case Step::NOTHING: return out;
        case Step::USE: out.paired = true; return out;

        case Step::FETCH_ROOT: {
          const home_display_ports::RootReply reply = est_.fetch_root();
          if (reply.result != home_display_ports::Fetch::OK) {
            out.failure = home_display_report::Failure::SERVER;
            return out;
          }
          provisional = reply.root;
          break;
        }

        case Step::MAKE_KEY:
          if (!identity_.make_key()) {
            out.failure = home_display_report::Failure::MEMORY;  // nowhere to keep it
            return out;
          }
          break;

        case Step::ENROLL:
        case Step::RENEW: {
          out.code = code(root, name_, identity_.spki());
          const home_display_ports::Reply reply =
              n.action == Step::ENROLL ? est_.enroll(name_, identity_, root) : est_.renew(name_, identity_, root);
          switch (reply.result) {
            case home_display_ports::Result::UNREACHABLE:
              out.failure = home_display_report::Failure::SERVER;
              return out;
            case home_display_ports::Result::TLS_REFUSED:
              out.failure = home_display_report::Failure::CERTIFICATE;
              return out;
            case home_display_ports::Result::PENDING:
              identity_.set_last_answer(Answer::PENDING);
              out.standing = Standing::WAITING;
              out.failure = failure_for(out.standing);
              out.retry_after_s = reply.retry_after_s;
              return out;
            case home_display_ports::Result::REFUSED:
              identity_.set_last_answer(Answer::REFUSED);
              if (n.action == Step::ENROLL) {
                out.standing = Standing::NOT_RECOGNISED;
                out.failure = failure_for(out.standing);
                return out;
              }
              break;  // a refused renewal: next time round it is asked as a new display
            case home_display_ports::Result::ISSUED:
              if (!verifier_.chains_to(reply.certificate, reply.intermediates, root)) {
                out.failure = home_display_report::Failure::CERTIFICATE;  // not signed under the root: not kept
                return out;
              }
              // The root is kept first: the certificate is the proof it is the right one, and a root with no
              // certificate is harmless, where a certificate with no root could not be checked.
              if (!provisional.empty() && !identity_.save_root(provisional)) {
                out.failure = home_display_report::Failure::MEMORY;
                return out;
              }
              if (!identity_.save_certificate(reply.certificate, reply.lifetime)) {
                out.failure = home_display_report::Failure::MEMORY;
                return out;
              }
              identity_.set_last_answer(Answer::NONE);
              // Done for this wake, and not looked at again: a certificate that is already over, or not yet begun, by
              // this clock would otherwise be asked for again, and again.
              switch (home_display_trust::standing(clock_.now(), reply.lifetime.not_before, reply.lifetime.not_after)) {
                case home_display_trust::Standing::NOT_YET:
                  out.standing = Standing::CLOCK_NOT_SET;
                  out.failure = home_display_report::Failure::CLOCK;
                  return out;
                case home_display_trust::Standing::EXPIRED:
                  out.failure = home_display_report::Failure::CERTIFICATE;
                  return out;
                default:
                  out.standing = Standing::PAIRED;
                  out.failure = home_display_report::Failure::NONE;
                  out.paired = true;
                  return out;
              }
          }
          break;
        }
      }
    }
    return out;
  }

 private:
  home_display_ports::Clock &clock_;
  home_display_ports::Identity &identity_;
  home_display_ports::Est &est_;
  home_display_ports::Verifier &verifier_;
  std::string name_;
};

}  // namespace home_display_join
