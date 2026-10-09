// An HTTP/1.1 response taken apart as it arrives, in whatever pieces the network delivers it: first the head (status
// line and headers), then the body, framed by a Content-Length, by chunked transfer coding, or by the connection
// closing.
//
// The image is megabytes and the chip has some hundreds of KB, so the body is passed on as it comes and never held
// whole. eink_http.h does the same for the small replies that fit in a string; this is for the one that does not.
//
// Tested with every way the bytes can be split.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace eink_body {

struct Header {
  std::string name;  // lower case
  std::string value;
};

// How the body ends.
enum class Framing : uint8_t {
  LENGTH,   // Content-Length bytes
  CHUNKED,  // chunks, ending in a zero-length one
  CLOSE,    // when the connection closes (HTTP/1.0 style: no length, not chunked)
  NONE,     // no body: 1xx, 204, 304, and the reply to a HEAD
};

enum class State : uint8_t {
  HEAD,  // still reading the status line and headers
  BODY,
  DONE,    // the whole body has been passed on
  FAILED,  // not a response we can follow
};

class Response {
 public:
  // The longest head accepted. A reply from our server has a few hundred bytes of it; more is not our server.
  static constexpr size_t MAX_HEAD = 8192;

  State state() const { return state_; }
  int status() const { return status_; }
  Framing framing() const { return framing_; }
  // Content-Length when there is one, else 0 (see `has_length`).
  size_t content_length() const { return content_length_; }
  bool has_length() const { return framing_ == Framing::LENGTH; }
  const std::vector<Header> &headers() const { return headers_; }

  std::string header(const std::string &name) const {
    for (const Header &h : headers_)
      if (h.name == lower(name))
        return h.value;
    return "";
  }

  // Bytes of the body handed on so far.
  size_t body_bytes() const { return body_bytes_; }

  // Gives the next bytes that arrived, `in`. Body bytes are appended to `out`; head bytes are kept. Returns how many of
  // `in` were used: all of them unless the response ended (DONE) or failed, when the rest belongs to nothing here.
  size_t feed(const uint8_t *in, size_t length, std::string &out) {
    size_t used = 0;
    while (used < length && state_ != State::DONE && state_ != State::FAILED) {
      if (state_ == State::HEAD) {
        head_ += static_cast<char>(in[used++]);
        if (head_.size() > MAX_HEAD) {
          state_ = State::FAILED;
          break;
        }
        if (ends_head())
          finish_head();
      } else {
        used += body(in + used, length - used, out);
      }
    }
    return used;
  }

  // The connection closed. A body that ends that way is complete; any other is cut short.
  void closed() {
    if (state_ == State::BODY && framing_ == Framing::CLOSE)
      state_ = State::DONE;
    else if (state_ == State::HEAD || state_ == State::BODY)
      state_ = State::FAILED;
  }

  // A reply that cannot have a body, whatever its headers say.
  bool bodyless() const { return framing_ == Framing::NONE; }

 private:
  State state_ = State::HEAD;
  std::string head_;
  int status_ = 0;
  Framing framing_ = Framing::CLOSE;
  size_t content_length_ = 0;
  size_t body_bytes_ = 0;
  size_t remaining_ = 0;  // LENGTH: bytes still to come; CHUNKED: bytes left in this chunk
  std::vector<Header> headers_;

  // CHUNKED: where in a chunk the next byte is.
  enum class Chunk : uint8_t { SIZE, SIZE_LF, DATA, DATA_CR, DATA_LF, TRAILER, TRAILER_LF } chunk_ = Chunk::SIZE;
  size_t size_ = 0;
  bool size_digits_ = false;
  bool extension_ = false;  // after a ';' in a chunk size line
  bool trailer_empty_ = true;

  static std::string lower(std::string text) {
    for (char &c : text)
      if (c >= 'A' && c <= 'Z')
        c = static_cast<char>(c - 'A' + 'a');
    return text;
  }

  static std::string trim(const std::string &text) {
    size_t a = 0, b = text.size();
    while (a < b && (text[a] == ' ' || text[a] == '\t'))
      a++;
    while (b > a && (text[b - 1] == ' ' || text[b - 1] == '\t'))
      b--;
    return text.substr(a, b - a);
  }

  bool ends_head() const {
    const size_t n = head_.size();
    return n >= 4 && head_[n - 4] == '\r' && head_[n - 3] == '\n' && head_[n - 2] == '\r' && head_[n - 1] == '\n';
  }

  // The largest length or chunk accepted: 2 GB, which fits a 32-bit size_t (the chip's) without wrapping.
  static constexpr uint64_t MAX_BODY = 0x7fffffff;

  // Digits only, no sign, no overflow: a Content-Length is a number or it is not one. Worked out in 64 bits so that the
  // chip's 32-bit size_t cannot wrap it, and refused above MAX_BODY.
  static bool number(const std::string &text, size_t &out) {
    if (text.empty() || text.size() > 18)
      return false;
    uint64_t v = 0;
    for (char c : text) {
      if (c < '0' || c > '9')
        return false;
      v = v * 10 + static_cast<uint64_t>(c - '0');
    }
    if (v > MAX_BODY)
      return false;
    out = static_cast<size_t>(v);
    return true;
  }

  void finish_head() {
    // "HTTP/1.1 200 OK"
    const size_t eol = head_.find("\r\n");
    const std::string line = head_.substr(0, eol);
    if (line.compare(0, 5, "HTTP/") != 0) {
      state_ = State::FAILED;
      return;
    }
    const size_t space = line.find(' ');
    size_t status = 0;
    if (space == std::string::npos || !number(line.substr(space + 1, 3), status) || line.size() < space + 4 ||
        status < 100 || status > 599) {
      state_ = State::FAILED;
      return;
    }
    status_ = static_cast<int>(status);

    size_t at = eol + 2;
    while (at < head_.size()) {
      const size_t end = head_.find("\r\n", at);
      if (end == std::string::npos || end == at)
        break;
      const std::string field = head_.substr(at, end - at);
      const size_t colon = field.find(':');
      if (colon == std::string::npos || colon == 0) {
        state_ = State::FAILED;
        return;
      }
      headers_.push_back({lower(field.substr(0, colon)), trim(field.substr(colon + 1))});
      at = end + 2;
    }

    const std::string transfer = lower(header("transfer-encoding"));
    const std::string length = header("content-length");
    if ((status_ >= 100 && status_ < 200) || status_ == 204 || status_ == 304) {
      framing_ = Framing::NONE;
    } else if (!transfer.empty()) {
      // A body in any coding but plain chunked is one this cannot follow; so is a length beside it (the two together
      // are how a request is smuggled past a proxy, and nothing honest sends them).
      if (transfer != "chunked" || !length.empty()) {
        state_ = State::FAILED;
        return;
      }
      framing_ = Framing::CHUNKED;
    } else if (!length.empty()) {
      size_t n = 0;
      if (!number(length, n)) {
        state_ = State::FAILED;
        return;
      }
      framing_ = Framing::LENGTH;
      content_length_ = n;
      remaining_ = n;
    } else {
      framing_ = Framing::CLOSE;
    }
    head_.clear();
    head_.shrink_to_fit();
    state_ = framing_ == Framing::NONE || (framing_ == Framing::LENGTH && remaining_ == 0) ? State::DONE : State::BODY;
  }

  // Takes body bytes from `in`; returns how many it used.
  size_t body(const uint8_t *in, size_t length, std::string &out) {
    if (framing_ == Framing::LENGTH) {
      const size_t n = length < remaining_ ? length : remaining_;
      out.append(reinterpret_cast<const char *>(in), n);
      remaining_ -= n;
      body_bytes_ += n;
      if (remaining_ == 0)
        state_ = State::DONE;
      return n;
    }
    if (framing_ == Framing::CLOSE) {
      out.append(reinterpret_cast<const char *>(in), length);
      body_bytes_ += length;
      return length;
    }
    return chunked(in, length, out);
  }

  static int hex(uint8_t c) {
    if (c >= '0' && c <= '9')
      return c - '0';
    if (c >= 'a' && c <= 'f')
      return c - 'a' + 10;
    if (c >= 'A' && c <= 'F')
      return c - 'A' + 10;
    return -1;
  }

  size_t chunked(const uint8_t *in, size_t length, std::string &out) {
    size_t used = 0;
    while (used < length && state_ == State::BODY) {
      const uint8_t c = in[used];
      switch (chunk_) {
        case Chunk::SIZE:
          used++;
          if (c == '\r') {
            if (!size_digits_) {
              state_ = State::FAILED;
              break;
            }
            chunk_ = Chunk::SIZE_LF;
          } else if (c == ';') {
            extension_ = true;
          } else if (!extension_) {
            const int d = hex(c);
            if (d < 0 || size_ > (MAX_BODY >> 4)) {
              state_ = State::FAILED;
              break;
            }
            size_ = size_ * 16 + static_cast<size_t>(d);
            size_digits_ = true;
          }
          break;
        case Chunk::SIZE_LF:
          used++;
          if (c != '\n') {
            state_ = State::FAILED;
            break;
          }
          remaining_ = size_;
          chunk_ = size_ == 0 ? Chunk::TRAILER : Chunk::DATA;
          trailer_empty_ = true;
          break;
        case Chunk::DATA: {
          const size_t n = length - used < remaining_ ? length - used : remaining_;
          out.append(reinterpret_cast<const char *>(in + used), n);
          used += n;
          remaining_ -= n;
          body_bytes_ += n;
          if (remaining_ == 0)
            chunk_ = Chunk::DATA_CR;
          break;
        }
        case Chunk::DATA_CR:
          used++;
          if (c != '\r') {
            state_ = State::FAILED;
            break;
          }
          chunk_ = Chunk::DATA_LF;
          break;
        case Chunk::DATA_LF:
          used++;
          if (c != '\n') {
            state_ = State::FAILED;
            break;
          }
          size_ = 0;
          size_digits_ = false;
          extension_ = false;
          chunk_ = Chunk::SIZE;
          break;
        case Chunk::TRAILER:  // after the last chunk: trailer lines, then an empty line
          used++;
          if (c == '\r') {
            chunk_ = Chunk::TRAILER_LF;
          } else {
            trailer_empty_ = false;
          }
          break;
        case Chunk::TRAILER_LF:
          used++;
          if (c != '\n') {
            state_ = State::FAILED;
            break;
          }
          if (trailer_empty_) {
            state_ = State::DONE;
          } else {
            trailer_empty_ = true;
            chunk_ = Chunk::TRAILER;
          }
          break;
      }
    }
    return used;
  }
};

}  // namespace eink_body
