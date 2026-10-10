// A flash stand-in for tests: named blobs in memory, with ways to fail, so a test can cut a write short, make a read
// fail, or leave a blob damaged.
#pragma once

#include <map>
#include <string>
#include <vector>

#include "home_display/core/credentials.h"

namespace fakes {

struct MemoryStore : home_display_credentials::BlobStore {
  std::map<std::string, home_display_credentials::Bytes> blobs;
  int writes = 0;

  // The Nth write from now (1 is the next) is cut short: only the first half of the blob is kept, and the power is
  // gone, so that write and every one after it fails until `restore_power()`.
  int cut_write = 0;
  bool power_off = false;
  // A write that fails outright, leaving the blob as it was.
  int fail_write = 0;
  // Reads of these names fail.
  std::map<std::string, bool> broken_reads;

  home_display_credentials::Read read(const char *name, home_display_credentials::Bytes &out) override {
    if (broken_reads[name])
      return home_display_credentials::Read::ERROR;
    const auto found = blobs.find(name);
    if (found == blobs.end())
      return home_display_credentials::Read::ABSENT;
    out = found->second;
    return home_display_credentials::Read::OK;
  }

  bool write(const char *name, const home_display_credentials::Bytes &data) override {
    if (power_off)
      return false;
    writes++;
    if (cut_write > 0 && --cut_write == 0) {
      blobs[name] = home_display_credentials::Bytes(data.begin(), data.begin() + static_cast<long>(data.size() / 2));
      power_off = true;
      return false;
    }
    if (fail_write > 0 && --fail_write == 0)
      return false;
    blobs[name] = data;
    return true;
  }

  // Erases fail the same ways writes do: the Nth from now loses the power before it (leaving the blob), or fails
  // outright.
  int cut_erase = 0;
  int fail_erase = 0;
  std::vector<std::string> erased;  // in order, for tests of what goes first

  bool erase(const char *name) override {
    if (power_off)
      return false;
    if (cut_erase > 0 && --cut_erase == 0) {
      power_off = true;
      return false;
    }
    if (fail_erase > 0 && --fail_erase == 0)
      return false;
    erased.push_back(name);
    blobs.erase(name);
    return true;
  }

  void restore_power() { power_off = false; }
};

}  // namespace fakes
