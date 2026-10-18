// The places where mbedTLS 3.6 (ESP-IDF 5.5) and mbedTLS 4 (ESP-IDF 6.1) differ, behind one name each, so that the rest of
// the TLS code is the same for both and carries no `#if`.
//
// What changed in 4, as far as this code is concerned:
//  - The cryptography is PSA (psa/crypto.h). It has to be started (`psa_crypto_init`) before anything uses it, and it has
//    its own random numbers and hashes: there is no entropy or DRBG module any more, no `mbedtls_sha256`, and no random
//    callback to give to `mbedtls_ssl_conf_rng`, `mbedtls_pk_sign` or `mbedtls_pk_parse_key`.
//  - A key is made by PSA, and copied into a pk context if it has to be written out: there is no `mbedtls_pk_setup` with an
//    EC key, and no `mbedtls_ecp_gen_key`.
//  - A TLS 1.3 client is always told when a session ticket arrives (`MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET`); in 3.6
//    it had to ask first.
//
// Everything is inline and header-only, like the rest of tls/, and nothing here throws.
#pragma once

#include <cstddef>
#include <cstdint>

#include "mbedtls/build_info.h"
#include "mbedtls/md.h"
#include "mbedtls/pk.h"
#include "mbedtls/ssl.h"

#if MBEDTLS_VERSION_MAJOR >= 4
#define HOME_DISPLAY_MBEDTLS_PSA 1
#include "psa/crypto.h"
#else
#define HOME_DISPLAY_MBEDTLS_PSA 0
#include "mbedtls/ctr_drbg.h"
#include "mbedtls/ecp.h"
#include "mbedtls/entropy.h"
#include "mbedtls/sha256.h"
#endif

namespace home_display_tls {
namespace compat {

#if HOME_DISPLAY_MBEDTLS_PSA

// Starts the cryptography, once. False if it cannot be started, in which case nothing that needs it can work.
inline bool init() {
  static const bool started = psa_crypto_init() == PSA_SUCCESS;
  return started;
}

// `length` random bytes.
inline bool random(unsigned char *out, size_t length) { return init() && psa_generate_random(out, length) == PSA_SUCCESS; }

// The SHA-256 of `data`, 32 bytes. 0 if it worked.
inline int sha256(const unsigned char *data, size_t length, unsigned char out[32]) {
  size_t written = 0;
  return init() && psa_hash_compute(PSA_ALG_SHA_256, data, length, out, 32, &written) == PSA_SUCCESS && written == 32 ? 0
                                                                                                                      : -1;
}

// The library takes its random numbers from PSA, so there is nothing to give it.
inline void use_random(mbedtls_ssl_config *) {}

// A TLS 1.3 client is told about every ticket without asking.
inline void ask_for_tickets(mbedtls_ssl_config *) {}

// Loads a private key from its DER (SEC1 or PKCS #8). 0 if it worked.
inline int parse_private_key(mbedtls_pk_context *key, const unsigned char *der, size_t length) {
  return init() ? mbedtls_pk_parse_key(key, der, length, nullptr, 0) : -1;
}

// Signs a SHA-256 hash with the key, as a DER ECDSA signature. 0 if it worked.
inline int sign_sha256(mbedtls_pk_context *key, const unsigned char hash[32], unsigned char *signature,
                       size_t capacity, size_t *length) {
  return init() ? mbedtls_pk_sign(key, MBEDTLS_MD_SHA256, hash, 32, signature, capacity, length) : -1;
}

// Makes a P-256 key in `key`, which must be freshly initialised. The key is made by PSA and copied in, so that it can be
// written out (it is exportable for that and nothing else is done with the PSA copy, which is destroyed at once).
inline bool generate_p256_key(mbedtls_pk_context *key) {
  if (!init())
    return false;
  psa_key_attributes_t attributes = PSA_KEY_ATTRIBUTES_INIT;
  psa_set_key_type(&attributes, PSA_KEY_TYPE_ECC_KEY_PAIR(PSA_ECC_FAMILY_SECP_R1));
  psa_set_key_bits(&attributes, 256);
  psa_set_key_usage_flags(&attributes, PSA_KEY_USAGE_EXPORT | PSA_KEY_USAGE_SIGN_HASH | PSA_KEY_USAGE_SIGN_MESSAGE);
  psa_set_key_algorithm(&attributes, PSA_ALG_ECDSA(PSA_ALG_SHA_256));
  mbedtls_svc_key_id_t id = MBEDTLS_SVC_KEY_ID_INIT;
  if (psa_generate_key(&attributes, &id) != PSA_SUCCESS)
    return false;
  const int copied = mbedtls_pk_copy_from_psa(id, key);
  psa_destroy_key(id);
  return copied == 0;
}

#else  // mbedTLS 3.6

// The random numbers for keys, handshakes and signatures. One for the program.
class Random {
 public:
  static Random &get() {
    static Random instance;
    return instance;
  }
  static int generate(void *, unsigned char *out, size_t length) {
    return mbedtls_ctr_drbg_random(&get().drbg_, out, length);
  }

 private:
  Random() {
    mbedtls_entropy_init(&entropy_);
    mbedtls_ctr_drbg_init(&drbg_);
    mbedtls_ctr_drbg_seed(&drbg_, mbedtls_entropy_func, &entropy_, nullptr, 0);
  }
  mbedtls_entropy_context entropy_;
  mbedtls_ctr_drbg_context drbg_;
};

inline bool init() {
  Random::get();
  return true;
}

inline bool random(unsigned char *out, size_t length) { return Random::generate(nullptr, out, length) == 0; }

inline int sha256(const unsigned char *data, size_t length, unsigned char out[32]) {
  return mbedtls_sha256(data, length, out, 0);
}

inline void use_random(mbedtls_ssl_config *conf) { mbedtls_ssl_conf_rng(conf, Random::generate, nullptr); }

// In TLS 1.3 a ticket arrives after the handshake; the library keeps it only if the application asks to be told.
inline void ask_for_tickets(mbedtls_ssl_config *conf) {
  mbedtls_ssl_conf_tls13_enable_signal_new_session_tickets(conf, MBEDTLS_SSL_TLS1_3_SIGNAL_NEW_SESSION_TICKETS_ENABLED);
}

inline int parse_private_key(mbedtls_pk_context *key, const unsigned char *der, size_t length) {
  return mbedtls_pk_parse_key(key, der, length, nullptr, 0, Random::generate, nullptr);
}

inline int sign_sha256(mbedtls_pk_context *key, const unsigned char hash[32], unsigned char *signature,
                       size_t capacity, size_t *length) {
  return mbedtls_pk_sign(key, MBEDTLS_MD_SHA256, hash, 32, signature, capacity, length, Random::generate, nullptr);
}

inline bool generate_p256_key(mbedtls_pk_context *key) {
  return mbedtls_pk_setup(key, mbedtls_pk_info_from_type(MBEDTLS_PK_ECKEY)) == 0 &&
         mbedtls_ecp_gen_key(MBEDTLS_ECP_DP_SECP256R1, mbedtls_pk_ec(*key), Random::generate, nullptr) == 0;
}

#endif

}  // namespace compat
}  // namespace home_display_tls
