#include <string>

#include "check.h"
#include "home_display/core/der.h"

using namespace home_display_der;

TEST(lengths_are_written_in_the_shortest_form_der_allows) {
  CHECK(tlv(0x04, Bytes(0)) == (Bytes{0x04, 0x00}));
  CHECK(tlv(0x04, Bytes(127))[1] == 0x7f);
  const Bytes a = tlv(0x04, Bytes(128));
  CHECK(a[1] == 0x81 && a[2] == 0x80 && a.size() == 131);
  const Bytes b = tlv(0x04, Bytes(255));
  CHECK(b[1] == 0x81 && b[2] == 0xff);
  const Bytes c = tlv(0x04, Bytes(256));
  CHECK(c[1] == 0x82 && c[2] == 0x01 && c[3] == 0x00);
  const Bytes d = tlv(0x04, Bytes(65536));
  CHECK(d[1] == 0x83 && d[2] == 0x01 && d[3] == 0x00 && d[4] == 0x00);
}

TEST(what_is_written_can_be_read_back) {
  for (size_t n : {0u, 1u, 127u, 128u, 255u, 256u, 1000u, 65535u, 65536u, 70000u}) {
    Bytes value(n, 0xAB);
    const Bytes encoded = tlv(0x30, value);
    const Tlv t = read(encoded.data(), encoded.size());
    CHECK(t.ok);
    CHECK_EQ(t.tag, (uint8_t) 0x30);
    CHECK_EQ(t.length, n);
    CHECK_EQ(t.total, encoded.size());
  }
}

TEST(a_cut_short_or_lying_element_is_not_ok) {
  const Bytes whole = tlv(0x04, Bytes(10, 1));
  for (size_t cut = 0; cut < whole.size(); cut++)
    CHECK(!read(whole.data(), cut).ok);
  const Bytes lies = {0x04, 0x84, 0xff, 0xff, 0xff, 0xff, 0x00};  // claims 4 GB
  CHECK(!read(lies.data(), lies.size()).ok);
  const Bytes wide = {0x04, 0x85, 0, 0, 0, 0, 1};  // a length wider than four bytes
  CHECK(!read(wide.data(), wide.size()).ok);
  const Bytes indefinite = {0x30, 0x80, 0x00, 0x00};  // BER, not DER
  CHECK(!read(indefinite.data(), indefinite.size()).ok);
}

// A CMS message the way the server writes it, with stand-ins for the certificates.
static Bytes cms(const std::vector<Bytes> &certificates) {
  Bytes set;
  for (const Bytes &c : certificates)
    set.insert(set.end(), c.begin(), c.end());
  const Bytes signed_oid = {0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02};
  const Bytes data_oid = {0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01};
  const Bytes signed_data = tlv(0x30, concat({Bytes{0x02, 0x01, 0x01},  // version
                                              tlv(0x31, Bytes()),       // digestAlgorithms
                                              tlv(0x30, data_oid),      // encapContentInfo
                                              tlv(0xA0, set),           // certificates
                                              tlv(0x31, Bytes())}));    // signerInfos
  return tlv(0x30, concat({signed_oid, tlv(0xA0, signed_data)}));
}

static Bytes fake_certificate(uint8_t seed, size_t size) { return tlv(0x30, Bytes(size, seed)); }

TEST(the_certificates_come_out_of_a_certs_only_message_in_order) {
  const Bytes root = fake_certificate(1, 300), intermediate = fake_certificate(2, 40);
  const std::vector<Bytes> found = certificates_in(cms({root, intermediate}));
  CHECK_EQ(found.size(), (size_t) 2);
  CHECK(found[0] == root);
  CHECK(found[1] == intermediate);
}

TEST(a_message_with_one_certificate_gives_one) {
  const Bytes leaf = fake_certificate(7, 500);
  const std::vector<Bytes> found = certificates_in(cms({leaf}));
  CHECK_EQ(found.size(), (size_t) 1);
  CHECK(found[0] == leaf);
}

TEST(a_message_with_none_gives_none) { CHECK(certificates_in(cms({})).empty()); }

TEST(something_that_is_not_such_a_message_gives_none) {
  CHECK(certificates_in(Bytes()).empty());
  CHECK(certificates_in(Bytes{0x04, 0x00}).empty());
  CHECK(certificates_in(tlv(0x30, Bytes{0x01})).empty());
  // Every cut of a good message: never a certificate that is not whole.
  const Bytes whole = cms({fake_certificate(1, 100), fake_certificate(2, 100)});
  for (size_t cut = 0; cut < whole.size(); cut++) {
    const std::vector<Bytes> found = certificates_in(Bytes(whole.begin(), whole.begin() + cut));
    CHECK(found.empty());
  }
}

TEST(the_request_has_the_parts_the_server_reads) {
  const Bytes spki = tlv(0x30, Bytes(89, 3));
  const Bytes info = request_info("kitchen", spki, "BASE64TEXT");
  // SEQUENCE { INTEGER 0, SEQUENCE { SET { SEQUENCE { commonName, UTF8String } } }, SPKI, [0] { attribute } }
  Tlv outer = read(info.data(), info.size());
  CHECK(outer.ok && outer.tag == 0x30 && outer.total == info.size());
  const uint8_t *p = outer.value;
  size_t left = outer.length;
  const Tlv version = read(p, left);
  CHECK(version.ok && version.tag == 0x02 && version.length == 1 && version.value[0] == 0);
  p += version.total, left -= version.total;
  const Tlv subject = read(p, left);
  CHECK(subject.ok && subject.tag == 0x30);
  const std::string subject_text(reinterpret_cast<const char *>(subject.value), subject.length);
  CHECK(subject_text.find("kitchen") != std::string::npos);
  CHECK(subject_text.find(std::string("\x55\x04\x03", 3)) != std::string::npos);  // commonName
  p += subject.total, left -= subject.total;
  const Tlv key = read(p, left);
  CHECK(key.ok && Bytes(p, p + key.total) == spki);
  p += key.total, left -= key.total;
  const Tlv attributes = read(p, left);
  CHECK(attributes.ok && attributes.tag == 0xA0 && attributes.total == left);
  const std::string attribute_text(reinterpret_cast<const char *>(attributes.value), attributes.length);
  CHECK(attribute_text.find("BASE64TEXT") != std::string::npos);
  CHECK(attribute_text.find(std::string("\x2A\x86\x48\x86\xF7\x0D\x01\x09\x07", 9)) != std::string::npos);
}

TEST(with_no_challenge_password_the_attributes_are_empty) {
  const Bytes info = request_info("kitchen", tlv(0x30, Bytes(2, 0)), "");
  CHECK_EQ(info.back(), (uint8_t) 0x00);  // [0] with nothing in it
  CHECK_EQ(info[info.size() - 2], (uint8_t) 0xA0);
}

TEST(the_finished_request_is_the_info_the_algorithm_and_the_signature) {
  const Bytes info = request_info("kitchen", tlv(0x30, Bytes(2, 0)), "x");
  const Bytes signature = {0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x02};
  const Bytes finished = request(info, signature);
  const Tlv outer = read(finished.data(), finished.size());
  CHECK(outer.ok && outer.tag == 0x30 && outer.total == finished.size());
  CHECK(Bytes(outer.value, outer.value + info.size()) == info);
  const uint8_t *p = outer.value + info.size();
  CHECK(Bytes(p, p + ecdsa_with_sha256().size()) == ecdsa_with_sha256());
  p += ecdsa_with_sha256().size();
  const Tlv bits = read(p, outer.value + outer.length - p);
  CHECK(bits.ok && bits.tag == 0x03 && bits.value[0] == 0x00);
  CHECK(Bytes(bits.value + 1, bits.value + bits.length) == signature);
}
