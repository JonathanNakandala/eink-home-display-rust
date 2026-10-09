// Which of the servers a scan found is the one to use, and where it is, from what it announced.
//
// The scan itself (mDNS) is eink_discovery.h, which fills an `Announcement` from each answer and asks here.
//
// What the server announces is in src/adapters/image_server/advertise.rs: the TXT keys txtvers, path, format and
// formats; and, when it offers HTTPS, tlsport (where) and secure (`optional` if plain HTTP is served too, `required`
// if not). With `transport = "https"` it is announced as `_https._tcp` on the HTTPS port and nowhere under `_http`.
#pragma once

#include <cstdint>
#include <string>

namespace eink_service {

// How the display is set to reach the server (the `server_transport` substitution). Only HTTPS is never downgraded.
enum class Transport : uint8_t {
  HTTP,          // plain HTTP only: ignores what the server offers
  PREFER_HTTPS,  // HTTPS when the server offers it and the display has joined, else HTTP
  HTTPS,         // HTTPS only: never HTTP, whatever is announced
};

// The DNS-SD service type to browse. A display set to HTTPS only looks under `_https` and nowhere else, since that is
// the only place a TLS-only server appears; the others look under `_http`, as the server announces itself there.
inline const char *service_type(Transport transport) { return transport == Transport::HTTPS ? "_https" : "_http"; }

// For a build that has to refuse a setting it does not know. Evaluated at compile time (`constexpr auto t = ...`), the
// call to a function with no body makes an unknown value a build error, not a quiet downgrade to plain HTTP.
void unknown_transport();

constexpr bool same(const char *a, const char *b) {
  while (*a != '\0' && *a == *b) {
    a++;
    b++;
  }
  return *a == *b;
}

constexpr Transport transport_from(const char *name) {
  if (same(name, "http"))
    return Transport::HTTP;
  if (same(name, "prefer-https"))
    return Transport::PREFER_HTTPS;
  if (same(name, "https"))
    return Transport::HTTPS;
  unknown_transport();
  return Transport::HTTPS;
}

// What one answer to the scan said, as text: its TXT values (empty if absent), its port and its IPv4 address.
struct Announcement {
  std::string instance;
  uint16_t port = 0;  // where the service is, from the SRV record
  uint32_t ip = 0;    // IPv4 in the byte order lwIP stores it (first octet in the lowest byte); 0 if none
  std::string txtvers;
  std::string path;
  std::string format;   // what the server sends now: "bmp", "png" or "qoi"; for the log only
  std::string tlsport;  // where HTTPS is, if offered
  std::string secure;   // "optional" or "required"
};

// The server to use, and how to reach it.
struct Server {
  uint32_t ip = 0;
  uint16_t port = 0;             // plain HTTP (0 if the server does not serve it)
  uint16_t tls_port = 0;         // HTTPS (0 if not offered)
  bool secure_required = false;  // the server serves no plain HTTP
  std::string format;
};

// `text` as a port number, 1 to 65535, or 0 if it is anything else (empty, a sign, a letter, too large).
inline uint16_t port_from(const std::string &text) {
  if (text.empty() || text.size() > 5)
    return 0;
  uint32_t value = 0;
  for (char c : text) {
    if (c < '0' || c > '9')
      return 0;
    value = value * 10 + (c - '0');
  }
  return value >= 1 && value <= 65535 ? static_cast<uint16_t>(value) : 0;
}

// Whether `answer` is our server (it says so in its TXT record: plenty on a LAN is `_http._tcp`), the one wanted
// (`instance_name`, when not empty, picks one of several) and usable, and if so what it is. `transport` is how the
// display was set to reach it, which decides what the answer's port means.
inline bool accept(const Announcement &answer, const std::string &instance_name, Transport transport, Server &server) {
  if (answer.txtvers != "1" || answer.path.empty())
    return false;
  if (!instance_name.empty() && answer.instance != instance_name)
    return false;
  if (answer.port == 0 || answer.ip == 0)
    return false;

  server = Server();
  server.ip = answer.ip;
  server.format = answer.format;
  server.secure_required = answer.secure == "required";
  const uint16_t announced = port_from(answer.tlsport);
  if (transport == Transport::HTTPS) {
    // Announced under `_https`, so the port is HTTPS. `tlsport` says the same; if it disagrees the announcement is
    // not to be trusted, and the display would be connecting somewhere the server did not say.
    if (announced != 0 && announced != answer.port)
      return false;
    server.tls_port = answer.port;
  } else {
    server.port = answer.port;
    server.tls_port = announced;
  }
  return true;
}

}  // namespace eink_service
