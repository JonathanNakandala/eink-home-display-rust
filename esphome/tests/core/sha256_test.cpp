#include <string>
#include <vector>

#include "check.h"
#include "home_display/core/sha256.h"

using namespace home_display_sha256;

// The examples in FIPS 180-4 / NIST CSRC "SHA-256 example values", and the empty string.
TEST(the_empty_string_and_the_standards_examples) {
  CHECK_EQ(hex(of(std::string(""))), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
  CHECK_EQ(hex(of(std::string("abc"))), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  CHECK_EQ(hex(of(std::string("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"))),
           "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
}

TEST(a_million_letters_a) {
  Hash hash;
  const std::string chunk(1000, 'a');
  for (int i = 0; i < 1000; i++)
    hash.update(chunk);
  CHECK_EQ(hex(hash.finish()), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
}

TEST(the_input_in_pieces_gives_the_same_digest_as_all_at_once) {
  const std::string text = "The quick brown fox jumps over the lazy dog";
  CHECK_EQ(hex(of(text)), "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592");
  for (size_t cut = 0; cut <= text.size(); cut++) {
    Hash hash;
    hash.update(text.substr(0, cut));
    hash.update(text.substr(cut));
    CHECK_EQ(hex(hash.finish()), "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592");
  }
}

TEST(the_lengths_around_a_block_edge_all_work) {
  // 55, 56, 63, 64 and 65 bytes straddle where the padding no longer fits in the block.
  CHECK_EQ(hex(of(std::string(55, 'a'))), "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318");
  CHECK_EQ(hex(of(std::string(56, 'a'))), "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a");
  CHECK_EQ(hex(of(std::string(63, 'a'))), "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34");
  CHECK_EQ(hex(of(std::string(64, 'a'))), "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb");
  CHECK_EQ(hex(of(std::string(65, 'a'))), "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0");
}

TEST(a_hash_object_can_be_used_again_after_finish) {
  Hash hash;
  hash.update("abc");
  hash.finish();
  hash.update("abc");
  CHECK_EQ(hex(hash.finish()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

TEST(a_length_is_eight_bytes_big_endian) {
  Hash a, b;
  a.update_length(0x0102030405060708ull);
  const std::vector<uint8_t> bytes = {1, 2, 3, 4, 5, 6, 7, 8};
  b.update(bytes);
  CHECK_EQ(hex(a.finish()), hex(b.finish()));
}
