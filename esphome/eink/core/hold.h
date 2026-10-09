// Holding a button down to reset the display's pairing: nothing happens for the first five seconds, then the panel says
// to keep holding, and at ten seconds the pairing is erased. Let go before that and nothing is reset.
//
// The button is read when the display wakes, so a press that is already over, as for a refresh, costs nothing. The
// timing is a calculation over what is read and when, and the YAML only reads the button and acts on the answer.
#pragma once

#include <cstdint>
#include <string>

namespace eink_hold {

constexpr uint32_t WARN_MS = 5000;    // the panel says to keep holding
constexpr uint32_t RESET_MS = 10000;  // the pairing is erased
// A button that reads up for less than this is bouncing, not let go.
constexpr uint32_t RELEASE_MS = 300;

enum class Action : uint8_t {
  NONE,
  WARN,    // held for WARN_MS: say so on the panel (once)
  RESET,   // held for RESET_MS: erase the pairing
  CANCEL,  // let go after the warning and before the reset: the warning is on the panel and has to go
};

class Tracker {
 public:
  // At the start of the wake: `down` is whether the button is held now. If it is not, there is nothing to track.
  void begin(uint32_t now_ms, bool down) {
    active_ = down;
    warned_ = false;
    up_ = false;
    start_ms_ = now_ms;
  }

  // Whether the button is still being followed: it was held at the start and has neither reached the reset nor been let
  // go. The wake waits for this to end before it goes on.
  bool active() const { return active_; }

  // What to do now. Call about every tenth of a second while `active()`. The times are a 32-bit millisecond counter
  // that wraps; the differences are right across a wrap.
  Action update(uint32_t now_ms, bool down) {
    if (!active_)
      return Action::NONE;
    if (down) {
      up_ = false;
      const uint32_t held = now_ms - start_ms_;
      if (held >= RESET_MS) {
        active_ = false;
        return Action::RESET;
      }
      if (held >= WARN_MS && !warned_) {
        warned_ = true;
        return Action::WARN;
      }
      return Action::NONE;
    }
    if (!up_) {
      up_ = true;
      up_since_ms_ = now_ms;
    }
    if (now_ms - up_since_ms_ < RELEASE_MS)
      return Action::NONE;
    active_ = false;
    return warned_ ? Action::CANCEL : Action::NONE;
  }

 private:
  bool active_ = false;
  bool warned_ = false;
  bool up_ = false;
  uint32_t start_ms_ = 0;
  uint32_t up_since_ms_ = 0;
};

// What the panel says. Plain words and the letters the notice font has (core/format.h, display.yaml).
inline std::string warning_notice() {
  return "Keep holding to reset pairing (" + std::to_string((RESET_MS - WARN_MS) / 1000) + " s)";
}

inline std::string done_notice(bool erased) { return erased ? "Pairing reset" : "Could not reset pairing"; }

}  // namespace eink_hold
