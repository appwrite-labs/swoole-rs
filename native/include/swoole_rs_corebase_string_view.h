/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_string_slot.h"
#include <cstdlib>
#include <cstring>
#include <new>
#include <string>
namespace swoole {
struct Socks5Proxy;
struct HttpProxy;
class Server;
struct ListenPort;
namespace coroutine {
class Socket;
}
// A native view over a Rust value. Copy construction clones bytes, never the
// view; std::string conversions copy the bytes out.
class RustStringView {
  friend struct Socks5Proxy;
  friend struct HttpProxy;
  friend class coroutine::Socket;
  friend class Server;
  friend struct ListenPort;
  const swr_v1_core_string *slot_;
  bool owned_ = false;
  explicit RustStringView(const swr_v1_core_string *slot) : slot_(slot) {}

public:
  RustStringView(const RustStringView &source)
      : slot_(swr_v1_core_string_clone(source.slot_)), owned_(true) {
    if (!slot_)
      throw std::bad_alloc();
  }
  ~RustStringView() {
    if (owned_)
      swr_v1_core_string_free(const_cast<swr_v1_core_string *>(slot_));
  }
  void check(int status) const {
    if (status == -2)
      throw std::bad_alloc();
    if (status < 0)
      std::abort();
  }
  RustStringView &assign(const char *bytes, size_t length) {
    check(swr_v1_core_string_set(slot_, bytes, length));
    return *this;
  }
  RustStringView &operator=(const std::string &value) {
    return assign(value.data(), value.size());
  }
  RustStringView &operator=(const char *value) {
    return assign(value, std::strlen(value));
  }
  RustStringView &operator=(const RustStringView &value) {
    if (this != &value) {
      auto view = swr_v1_core_string_view(value.slot_);
      assign(view.data, view.length);
    }
    return *this;
  }
  const char *c_str() const { return swr_v1_core_string_view(slot_).data; }
  const char *data() const { return c_str(); }
  size_t length() const { return swr_v1_core_string_view(slot_).length; }
  size_t size() const { return length(); }
  bool empty() const { return length() == 0; }
  bool equals(const char *bytes, size_t count) const {
    auto value = swr_v1_core_string_view(slot_);
    return value.length == count &&
           (count == 0 || std::memcmp(value.data, bytes, count) == 0);
  }
  friend bool operator==(const RustStringView &left,
                         const RustStringView &right) {
    return left.equals(right.data(), right.size());
  }
  friend bool operator==(const RustStringView &left, const std::string &right) {
    return left.equals(right.data(), right.size());
  }
  friend bool operator==(const std::string &left, const RustStringView &right) {
    return right == left;
  }
  friend bool operator==(const RustStringView &left, const char *right) {
    return left.equals(right, std::strlen(right));
  }
  friend bool operator==(const char *left, const RustStringView &right) {
    return right == left;
  }
  friend bool operator!=(const RustStringView &left,
                         const RustStringView &right) {
    return !(left == right);
  }
  friend bool operator!=(const RustStringView &left, const std::string &right) {
    return !(left == right);
  }
  friend bool operator!=(const std::string &left, const RustStringView &right) {
    return !(left == right);
  }
  friend bool operator!=(const RustStringView &left, const char *right) {
    return !(left == right);
  }
  friend bool operator!=(const char *left, const RustStringView &right) {
    return !(left == right);
  }
  void clear() { assign("", 0); }
  std::string std_string() const { return std::string(data(), size()); }
  operator std::string() const { return std_string(); }
};
} // namespace swoole
