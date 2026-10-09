// Flash on the chip: ESP-IDF's non-volatile storage as the `BlobStore` eink_credentials.h keeps its record in.
//
// Only the chip has this; the record format, the two slots and every way a write can be cut short are tested on a
// computer in eink_credentials.h. NVS writes an entry as a whole and commits it, so a power cut leaves the old value or
// the new one; the checksum in the record is a second line behind that.
//
// A private namespace ("eink"), so nothing here can touch ESPHome's own settings and none of ESPHome's touches these.
// The key is not encrypted here: that is NVS encryption or flash encryption, which are the owner's to turn on (a
// device in someone's hand can be read otherwise).
#pragma once

#include "nvs.h"
#include "nvs_flash.h"

#include "eink_credentials.h"

namespace eink_nvs {

class NvsStore : public eink_credentials::BlobStore {
 public:
  ~NvsStore() override {
    if (open_)
      nvs_close(handle_);
  }

  // False if NVS could not be opened. It is never erased from here: ESPHome starts NVS and decides what to do about a
  // partition it cannot use, and erasing it would take pairing with it.
  bool open() {
    if (open_)
      return true;
    const esp_err_t started = nvs_flash_init();  // already started by ESPHome, which makes this a no-op
    if (started != ESP_OK)
      return false;
    open_ = nvs_open("eink", NVS_READWRITE, &handle_) == ESP_OK;
    return open_;
  }

  eink_credentials::Read read(const char *name, eink_credentials::Bytes &out) override {
    if (!open())
      return eink_credentials::Read::ERROR;
    size_t size = 0;
    esp_err_t r = nvs_get_blob(handle_, name, nullptr, &size);
    if (r == ESP_ERR_NVS_NOT_FOUND)
      return eink_credentials::Read::ABSENT;
    if (r != ESP_OK)
      return eink_credentials::Read::ERROR;
    out.resize(size);
    r = nvs_get_blob(handle_, name, out.data(), &size);
    if (r != ESP_OK || size != out.size())
      return eink_credentials::Read::ERROR;
    return eink_credentials::Read::OK;
  }

  bool write(const char *name, const eink_credentials::Bytes &data) override {
    if (!open())
      return false;
    return nvs_set_blob(handle_, name, data.data(), data.size()) == ESP_OK && nvs_commit(handle_) == ESP_OK;
  }

 private:
  nvs_handle_t handle_ = 0;
  bool open_ = false;
};

}  // namespace eink_nvs
