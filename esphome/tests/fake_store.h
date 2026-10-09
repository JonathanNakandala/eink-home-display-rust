// A flash stand-in for tests: named blobs in memory, with ways to fail, so a test can cut a write short, make a read fail,
// or leave a blob damaged.
#pragma once

#include <map>
#include <string>

#include "eink/core/credentials.h"

namespace fakes {

struct MemoryStore : eink_credentials::BlobStore {
  std::map<std::string, eink_credentials::Bytes> blobs;
  int writes = 0;

  // The Nth write from now (1 is the next) is cut short: only the first half of the blob is kept, and the power is gone, so
  // that write and every one after it fails until `restore_power()`.
  int cut_write = 0;
  bool power_off = false;
  // A write that fails outright, leaving the blob as it was.
  int fail_write = 0;
  // Reads of these names fail.
  std::map<std::string, bool> broken_reads;

  eink_credentials::Read read(const char *name, eink_credentials::Bytes &out) override {
    if (broken_reads[name])
      return eink_credentials::Read::ERROR;
    const auto found = blobs.find(name);
    if (found == blobs.end())
      return eink_credentials::Read::ABSENT;
    out = found->second;
    return eink_credentials::Read::OK;
  }

  bool write(const char *name, const eink_credentials::Bytes &data) override {
    if (power_off)
      return false;
    writes++;
    if (cut_write > 0 && --cut_write == 0) {
      blobs[name] = eink_credentials::Bytes(data.begin(), data.begin() + static_cast<long>(data.size() / 2));
      power_off = true;
      return false;
    }
    if (fail_write > 0 && --fail_write == 0)
      return false;
    blobs[name] = data;
    return true;
  }

  void restore_power() { power_off = false; }
};

}  // namespace fakes
