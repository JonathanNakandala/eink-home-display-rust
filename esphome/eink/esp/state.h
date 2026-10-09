// State one wake keeps between its scripts that has a type of its own.
//
// ESPHome `globals:` are declared before the `includes:` headers are, so a global can't be of a type a header
// defines (only `int`, `bool`, `std::string` and the like). These are plain C++ variables instead, one per wake:
// nothing here is kept across deep sleep (that is what `restore_value` globals and RTC memory are for), and each
// starts afresh at boot. The types, and what is done with them, are tested on a computer in the headers they come from.
#pragma once

#include "eink/core/battery.h"
#include "eink/core/hold.h"
#include "eink/core/report.h"
#include "eink/core/secure_wake.h"
#include <string>

namespace eink_state {

// Why this wake failed, once it has (the reasons are in core/report.h). NONE if it didn't.
inline eink_report::Failure fail_reason = eink_report::Failure::NONE;

// How charged the battery is, for the server and the log.
inline eink_battery::State battery_state = eink_battery::State::OK;

// What this wake does about the battery: carry on, sleep (still halted), or halt now and say so first.
inline eink_battery::Action battery_action = eink_battery::Action::CARRY_ON;

// What this wake asks the server for its plan (core/plan.h `target`): the path and query, made once by `check_plan` and
// used by whichever route sends it.
inline std::string plan_target;

// The picture: what the display asks the server with, if the panel shows just the last picture (esp/shown_etag.h), as
// `If-None-Match`; and the ETag of the one this wake downloaded, for the panel to keep once it is drawn. Both "" when
// there is none.
inline std::string image_condition;
inline std::string downloaded_etag;

// The reset button (core/hold.h): how long KEY0 has been held since the wake began, and what that last said to do.
inline eink_hold::Tracker reset_hold;
inline eink_hold::Action hold_action = eink_hold::Action::NONE;

// ---- the secure transport (core/secure_wake.h) ----------------------------------------------------------------------

// Whether this wake speaks TLS to the server: the display has joined and the transport is set to use it.
inline bool secure_active = false;

// How this wake reaches the server, and why not by TLS when it does not.
inline eink_secure_wake::Route route = eink_secure_wake::Route::PLAIN;
inline eink_report::Failure secure_failure = eink_report::Failure::NONE;

// The owner has to type the pairing code in at the server, so the panel shows it: what it says, and how long the server
// asked the display to wait before asking again.
inline bool prompt_wanted = false;
inline std::string prompt_text;
inline uint32_t wait_retry_s = 0;

}  // namespace eink_state
