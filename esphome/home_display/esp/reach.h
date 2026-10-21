// Whether something accepts a TCP connection at an address and port, within a time: the check that the server is still
// where it was (core/locate.h) before a request is spent on finding out by timing out.
//
// Only the connection, never a request: it is closed as soon as it is made. A server that is there accepts it at once
// (a few milliseconds on a LAN), and the time it costs when nothing is, is `timeout_ms` and no more, where a request
// would wait for its own much longer timeouts (15 s for TLS, 50 s for plain HTTP, which has to be long for the server
// to render).
#pragma once

#include <cerrno>
#include <cstdint>

#include <fcntl.h>
#include <netinet/in.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

namespace home_display_reach {

constexpr uint32_t CONNECT_MS = 3000;

// `ip` as lwIP stores it (the first octet in the lowest byte, which is network order). A refused connection is a no:
// the machine is there but the server is not on that port.
inline bool connects(uint32_t ip, uint16_t port, uint32_t timeout_ms = CONNECT_MS) {
  const int fd = ::socket(AF_INET, SOCK_STREAM, 0);
  if (fd < 0)
    return false;
  const int flags = ::fcntl(fd, F_GETFL, 0);
  ::fcntl(fd, F_SETFL, flags | O_NONBLOCK);

  struct sockaddr_in address;
  address.sin_family = AF_INET;
  address.sin_port = htons(port);
  address.sin_addr.s_addr = ip;
  for (auto &byte : address.sin_zero)
    byte = 0;

  bool open = false;
  const int started = ::connect(fd, reinterpret_cast<struct sockaddr *>(&address), sizeof address);
  if (started == 0) {
    open = true;
  } else if (errno == EINPROGRESS) {
    fd_set writable;
    FD_ZERO(&writable);
    FD_SET(fd, &writable);
    struct timeval wait;
    wait.tv_sec = static_cast<long>(timeout_ms / 1000);
    wait.tv_usec = static_cast<long>((timeout_ms % 1000) * 1000);
    if (::select(fd + 1, nullptr, &writable, nullptr, &wait) > 0) {
      int error = 0;
      socklen_t length = sizeof error;
      open = ::getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &length) == 0 && error == 0;
    }
  }
  ::close(fd);
  return open;
}

}  // namespace home_display_reach
