// Whether the clock can be believed enough to check a certificate against.
//
// A certificate is valid between two dates, so before TLS the display needs the time. After a power cut the
// clock starts at 1970 until SNTP sets it, and a clock that says 1970 would find every certificate "not yet
// valid"; one wrongly far ahead would find them expired. Neither is a reason to think the display is unpaired,
// so the answer to an unusable clock is "try again next wake", never anything that touches the pairing.
#pragma once

#include <cstdint>

namespace eink_clock {

// 2026-01-01 00:00:00 UTC. The server will not make a certificate from a clock earlier than this either
// (`earliest_plausible` in src/domain/services/clock.rs), so the two sides agree on what "not set" means.
constexpr int64_t EARLIEST_PLAUSIBLE = 1767225600;

// Whether `now` (seconds since 1970) is a time SNTP could have set, as opposed to one the chip started from.
inline bool plausible(int64_t now) { return now >= EARLIEST_PLAUSIBLE; }

// Whether the clock can be used. `newest_render` is the newest image version the display has been told of (the
// server's `version`, which is when it rendered, in seconds since 1970; 0 if none). A clock earlier than a time
// that has already happened is wrong even if it is past 2026, for example one a server of the wrong date set.
inline bool usable(int64_t now, uint32_t newest_render) { return plausible(now) && now >= (int64_t) newest_render; }

}  // namespace eink_clock
