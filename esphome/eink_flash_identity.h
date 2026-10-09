// The display's identity as it is kept in flash: the interface eink_join.h and eink_est_client.h use, over the record
// in eink_credentials.h. The key is a real mbedTLS key, loaded from the record and written to it when it is made.
//
// Written against mbedTLS 3.6 and tested on a computer (host/), with a stand-in for flash that can lose power in any
// write. The chip uses it with the NVS store in eink_nvs.h.
//
// What the server last said (waiting, turned away) is not kept here, and not in flash: it changes with every wake while
// a display is waiting, and flash does not take that many writes. The caller gives a place to keep it that survives
// deep sleep and not a power cut (RTC memory on the chip), and a display that loses it just asks again and is told
// again.
#pragma once

#include "mbedtls/ecp.h"
#include "mbedtls/pk.h"

#include "eink_credentials.h"
#include "eink_csr.h"
#include "eink_tls.h"

namespace eink_flash {

using Bytes = eink_tls::Bytes;

class FlashIdentity : public eink_tls::TlsIdentity {
 public:
  // `compiled_root` is the owner's root built into the firmware (`server_root`), or empty. `last_answer` is where to
  // keep what the server last said.
  FlashIdentity(eink_credentials::BlobStore &store, eink_pairing::Answer &last_answer, Bytes compiled_root = {})
      : credentials_(store), last_(last_answer), compiled_(std::move(compiled_root)) {
    mbedtls_pk_init(&key_);
    if (credentials_.state() == eink_credentials::Credentials::State::READY && !credentials_.record().key.empty())
      loaded_ = parse_key(credentials_.record().key);
  }
  ~FlashIdentity() override { mbedtls_pk_free(&key_); }
  FlashIdentity(const FlashIdentity &) = delete;
  FlashIdentity &operator=(const FlashIdentity &) = delete;

  // False if what is in flash cannot be read, and so is being left alone: the caller reports it (as memory). Also when
  // a key is there and cannot be loaded: it is not replaced by a new one.
  bool readable() override {
    return credentials_.state() != eink_credentials::Credentials::State::UNREADABLE &&
           (credentials_.record().key.empty() || loaded_);
  }

  Bytes compiled_root() override { return compiled_; }
  Bytes stored_root() override { return credentials_.record().root; }
  bool save_root(const Bytes &der) override {
    eink_credentials::Record next = credentials_.record();
    next.root = der;
    return credentials_.update(next);
  }

  bool has_key() override { return loaded_; }

  // Made once, and written to flash before anything else is done with it. Never replaces a key.
  bool make_key() override {
    if (loaded_)
      return true;
    if (!readable())
      return false;
    mbedtls_pk_context fresh;
    mbedtls_pk_init(&fresh);
    unsigned char der[200];
    bool ok =
        mbedtls_pk_setup(&fresh, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY)) == 0 &&
        mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(fresh), eink_tls::Random::generate, nullptr) == 0;
    int length = ok ? mbedtls_pk_write_key_der(&fresh, der, sizeof der) : -1;  // written at the end of the buffer
    mbedtls_pk_free(&fresh);
    if (length <= 0)
      return false;
    eink_credentials::Record next = credentials_.record();
    next.key = Bytes(der + sizeof der - length, der + sizeof der);
    if (!credentials_.update(next))
      return false;
    loaded_ = parse_key(next.key);
    return loaded_;
  }

  Bytes spki() override { return eink_csr::public_key(key_); }
  Bytes csr(const std::string &name, const Bytes &binding) override { return eink_csr::request(key_, name, binding); }
  mbedtls_pk_context &private_key() override { return key_; }

  bool has_certificate() override { return !credentials_.record().certificate.empty(); }
  Bytes held_certificate() override { return credentials_.record().certificate; }
  eink_ports::Lifetime lifetime() override {
    return {credentials_.record().not_before, credentials_.record().not_after};
  }
  bool save_certificate(const Bytes &der, const eink_ports::Lifetime &lifetime) override {
    eink_credentials::Record next = credentials_.record();
    next.certificate = der;
    next.not_before = lifetime.not_before;
    next.not_after = lifetime.not_after;
    return credentials_.update(next);
  }

  eink_pairing::Answer last_answer() override { return last_; }
  void set_last_answer(eink_pairing::Answer answer) override { last_ = answer; }

 private:
  eink_credentials::Credentials credentials_;
  eink_pairing::Answer &last_;
  Bytes compiled_;
  mbedtls_pk_context key_;
  bool loaded_ = false;

  bool parse_key(const Bytes &der) {
    return mbedtls_pk_parse_key(&key_, der.data(), der.size(), nullptr, 0, eink_tls::Random::generate, nullptr) == 0;
  }
};

}  // namespace eink_flash
