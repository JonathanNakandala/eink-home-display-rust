// What a scan of the network says (core/service.h): text from any machine that answers mDNS, which decides where the
// display connects. The input is cut at zero bytes into the instance name, the TXT values and the numbers.
#include <string>
#include <vector>

#include "fuzz.h"

#include "eink/core/service.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  std::vector<std::string> parts(1);
  for (size_t i = 0; i < size; i++) {
    if (data[i] == 0)
      parts.emplace_back();
    else
      parts.back() += static_cast<char>(data[i]);
  }
  parts.resize(8);
  eink_service::Announcement answer;
  answer.instance = parts[0];
  answer.txtvers = parts[1];
  answer.path = parts[2];
  answer.format = parts[3];
  answer.tlsport = parts[4];
  answer.secure = parts[5];
  answer.port =
      static_cast<uint16_t>(parts[6].size() * 257 + (parts[6].empty() ? 0 : static_cast<uint8_t>(parts[6][0])));
  answer.ip = static_cast<uint32_t>(parts[7].size() * 0x01010101u);

  // A port is 1 to 65535 and only digits.
  const uint16_t port = eink_service::port_from(parts[4]);
  if (port != 0) {
    FUZZ_REQUIRE(parts[4].size() <= 5);
    for (char c : parts[4])
      FUZZ_REQUIRE(c >= '0' && c <= '9');
  }

  for (const eink_service::Transport transport :
       {eink_service::Transport::HTTP, eink_service::Transport::PREFER_HTTPS, eink_service::Transport::HTTPS}) {
    eink_service::Server server;
    if (!eink_service::accept(answer, parts[0], transport, server))
      continue;
    // Only our own server, at an address and port it gave, and the port it says for the way asked.
    FUZZ_REQUIRE(answer.txtvers == "1" && !answer.path.empty());
    FUZZ_REQUIRE(server.ip != 0 && server.ip == answer.ip);
    if (transport == eink_service::Transport::HTTPS) {
      FUZZ_REQUIRE(server.tls_port == answer.port && server.port == 0);
      FUZZ_REQUIRE(port == 0 || port == answer.port);
    } else {
      FUZZ_REQUIRE(server.port == answer.port);
    }
  }
  return 0;
}
