// State one wake keeps between its scripts that has a type of its own.
//
// ESPHome `globals:` are declared before the `includes:` headers are, so a global can't be of a type a header
// defines (only `int`, `bool`, `std::string` and the like). These are plain C++ variables instead, one per wake:
// nothing here is kept across deep sleep (that is what `restore_value` globals and RTC memory are for), and each
// starts afresh at boot. The types, and what is done with them, are tested on a computer in the headers they come from.
#pragma once

#include "eink_battery.h"
#include "eink_report.h"

namespace eink_state {

// Why this wake failed, once it has: WIFI, SERVER (no usable /plan), DOWNLOAD, MEMORY, or TIMEOUT (the
// `wake_watchdog` ran out). NONE if it didn't.
inline eink_report::Failure fail_reason = eink_report::Failure::NONE;

// How charged the battery is, for the server and the log.
inline eink_battery::State battery_state = eink_battery::State::OK;

// What this wake does about the battery: carry on, sleep (still halted), or halt now and say so first.
inline eink_battery::Action battery_action = eink_battery::Action::CARRY_ON;

}  // namespace eink_state
