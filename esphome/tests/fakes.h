// In-memory stand-ins for the interfaces in core/ports.h, so joining runs on a computer. They record what was asked of them
// and answer as the test says.
#pragma once

#include <string>
#include <vector>

#include "eink/core/ports.h"

namespace fakes {

using namespace eink_ports;

struct FakeClock : Clock {
  int64_t time = 1791463200;
  bool good = true;
  int64_t now() override { return time; }
  bool usable() override { return good; }
};

struct FakeIdentity : Identity {
  Bytes compiled;
  Bytes stored;
  bool key = false;
  Bytes key_spki = {0x30, 0x59, 1, 2, 3};
  bool certificate = false;
  Lifetime life;
  Bytes certificate_der;
  eink_pairing::Answer last = eink_pairing::Answer::NONE;
  // What to refuse, to see how a failing flash is handled.
  bool readable_ = true;
  bool can_make_key = true, can_save_root = true, can_save_certificate = true;
  int keys_made = 0, roots_saved = 0, certificates_saved = 0;

  bool readable() override { return readable_; }
  Bytes compiled_root() override { return compiled; }
  Bytes stored_root() override { return stored; }
  bool save_root(const Bytes &der) override {
    if (!can_save_root)
      return false;
    stored = der;
    roots_saved++;
    return true;
  }
  bool has_key() override { return key; }
  bool make_key() override {
    if (!can_make_key)
      return false;
    key = true;
    keys_made++;
    return true;
  }
  Bytes spki() override { return key_spki; }
  Bytes csr(const std::string &, const Bytes &) override { return {0xC5}; }
  bool has_certificate() override { return certificate; }
  Bytes held_certificate() override { return certificate_der; }
  Lifetime lifetime() override { return life; }
  bool save_certificate(const Bytes &der, const Lifetime &lifetime) override {
    if (!can_save_certificate)
      return false;
    certificate = true;
    certificate_der = der;
    life = lifetime;
    certificates_saved++;
    return true;
  }
  eink_pairing::Answer last_answer() override { return last; }
  void set_last_answer(eink_pairing::Answer answer) override { last = answer; }
};

struct FakeEst : Est {
  RootReply root_reply;
  Reply enroll_reply, renew_reply;
  int roots_fetched = 0, enrolled = 0, renewed = 0;
  Bytes enrolled_against, renewed_against;

  RootReply fetch_root() override {
    roots_fetched++;
    return root_reply;
  }
  Reply enroll(const std::string &, Identity &, const Bytes &root) override {
    enrolled++;
    enrolled_against = root;
    return enroll_reply;
  }
  Reply renew(const std::string &, Identity &, const Bytes &root) override {
    renewed++;
    renewed_against = root;
    return renew_reply;
  }
  int requests() const { return roots_fetched + enrolled + renewed; }
};

struct FakeVerifier : Verifier {
  bool accepts = true;
  Bytes checked_against;
  std::vector<Bytes> checked_through;
  bool chains_to(const Bytes &, const std::vector<Bytes> &intermediates, const Bytes &root) override {
    checked_against = root;
    checked_through = intermediates;
    return accepts;
  }
};

}  // namespace fakes
