// The ETag of the picture on the panel, kept so that a new render that comes out the same can be told apart from one
// that does not: the display asks for the picture with `If-None-Match`, and a server that has the same bytes answers
// 304 with no body, so nothing is downloaded and the panel is not refreshed.
//
// A fixed-size slot, laid out to sit in RTC memory (it survives deep sleep and is lost with the power, which only costs
// one download). What is in it is checked before it is believed, since an update over the air leaves whatever the last
// firmware put there.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

namespace eink_etag {

constexpr uint32_t MAGIC = 0xE1B70004;
// The server's ETags are a quoted 32-digit hash, 34 characters; room for a longer one, but not for anything.
constexpr size_t CAPACITY = 64;

struct Slot {
  uint32_t magic;
  uint8_t length;
  char data[CAPACITY];
};

// What an ETag is made of (RFC 9110): visible ASCII, no space. Anything else is not one, and would not survive being
// put in a header.
inline bool valid(const std::string &etag) {
  if (etag.empty() || etag.size() > CAPACITY)
    return false;
  for (const char c : etag)
    if (c < 0x21 || c > 0x7E)
      return false;
  return true;
}

inline void clear(Slot &slot) {
  slot.magic = 0;
  slot.length = 0;
  for (char &c : slot.data)
    c = 0;
}

// Keeps `etag` as that of the picture now on the panel. One that is not an ETag (the server sent none, or the picture
// came some way that has none) leaves nothing, so nothing is asked about a picture whose bytes are not known.
inline void keep(Slot &slot, const std::string &etag) {
  clear(slot);
  if (!valid(etag))
    return;
  slot.magic = MAGIC;
  slot.length = static_cast<uint8_t>(etag.size());
  for (size_t i = 0; i < etag.size(); i++)
    slot.data[i] = etag[i];
}

// The ETag kept, or "" if there is none or the slot is not one this firmware wrote.
inline std::string kept(const Slot &slot) {
  if (slot.magic != MAGIC || slot.length == 0 || slot.length > CAPACITY)
    return "";
  const std::string etag(slot.data, slot.length);
  return valid(etag) ? etag : "";
}

// What to send as `If-None-Match` for the picture about to be fetched, or "" to ask without. Only when the panel shows
// that picture and nothing else (`panel_shows_only_the_picture`, eink_plan::shows_only_the_picture): a 304 means the
// panel is right as it is, which it would not be under a notice or a label that has to change.
inline std::string condition(const Slot &slot, bool panel_shows_only_the_picture) {
  return panel_shows_only_the_picture ? kept(slot) : "";
}

}  // namespace eink_etag
