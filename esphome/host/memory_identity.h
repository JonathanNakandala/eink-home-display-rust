// The display's identity kept in memory, for tests on a computer: a real key made by mbedTLS and everything else held
// in plain members. The chip's is the same interface over flash. Its key, request and public key are the real ones
// (tls/csr.h), so the server sees exactly what it would from the chip.
#pragma once

#include "mbedtls/ecp.h"
#include "mbedtls/pk.h"

#include "eink/tls/csr.h"
#include "eink/tls/tls.h"

namespace host {

class MemoryIdentity : public eink_tls::TlsIdentity {
 public:
  MemoryIdentity() { mbedtls_pk_init(&key_); }
  ~MemoryIdentity() override { mbedtls_pk_free(&key_); }

  eink_ports::Bytes compiled;
  eink_ports::Bytes stored;
  eink_ports::Bytes certificate_der;
  eink_ports::Lifetime life;
  bool has_cert = false;
  bool has_the_key = false;
  eink_pairing::Answer last = eink_pairing::Answer::NONE;

  bool readable() override { return true; }
  eink_ports::Bytes compiled_root() override { return compiled; }
  eink_ports::Bytes stored_root() override { return stored; }
  bool save_root(const eink_ports::Bytes &der) override {
    stored = der;
    return true;
  }

  bool has_key() override { return has_the_key; }
  bool make_key() override {
    if (mbedtls_pk_setup(&key_, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY)) != 0)
      return false;
    if (mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(key_), eink_tls::Random::generate, nullptr) != 0)
      return false;
    has_the_key = true;
    return true;
  }
  eink_ports::Bytes spki() override { return eink_csr::public_key(key_); }
  eink_ports::Bytes csr(const std::string &name, const eink_ports::Bytes &binding) override {
    return eink_csr::request(key_, name, binding);
  }
  mbedtls_pk_context &private_key() override { return key_; }

  bool has_certificate() override { return has_cert; }
  eink_ports::Bytes held_certificate() override { return certificate_der; }
  eink_ports::Lifetime lifetime() override { return life; }
  bool save_certificate(const eink_ports::Bytes &der, const eink_ports::Lifetime &lifetime) override {
    certificate_der = der;
    life = lifetime;
    has_cert = true;
    return true;
  }

  eink_pairing::Answer last_answer() override { return last; }
  void set_last_answer(eink_pairing::Answer answer) override { last = answer; }

 private:
  mbedtls_pk_context key_;
};

}  // namespace host
