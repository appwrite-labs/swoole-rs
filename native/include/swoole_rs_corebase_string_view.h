/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_string_slot.h"
#include <cstdlib>
#include <cstring>
#include <exception>
#include <memory>
#include <new>
#include <string>
namespace swoole {
struct Socks5Proxy;
struct HttpProxy;
struct GethostbynameRequest;
struct GetaddrinfoRequest;
class Server;
struct ListenPort;
namespace coroutine {
class Socket;
}
// A native view over a Rust value. Copy construction clones bytes, never the
// view. Only const std::string& compatibility getters allocate a native
// reference cache.
class RustStringView {
  friend struct Socks5Proxy;
  friend struct HttpProxy;
  friend struct GethostbynameRequest;
  friend struct GetaddrinfoRequest;
  friend class coroutine::Socket;
  friend class Server;
  friend struct ListenPort;
  const swr_v1_core_string *slot_;
  bool owned_ = false;
  struct Cache {
    std::string value;
    std::exception_ptr exception;
    explicit Cache(const char *bytes, size_t length) : value(bytes, length) {}
  };
  mutable std::unique_ptr<Cache> cache_;
  explicit RustStringView(const swr_v1_core_string *slot) : slot_(slot) {}
  static bool update(void *context, const char *bytes, size_t length) noexcept {
    auto *self = static_cast<RustStringView *>(context);
    self->cache_->exception = {};
    try {
      self->cache_->value.assign(bytes, length);
      return true;
    } catch (...) {
      self->cache_->exception = std::current_exception();
      return false;
    }
  }

public:
  RustStringView(const RustStringView &source)
      : slot_(swr_v1_core_string_clone(source.slot_)), owned_(true) {
    if (!slot_)
      throw std::bad_alloc();
  }
  ~RustStringView() {
    if (cache_)
      swr_v1_core_string_cache(slot_, nullptr, nullptr);
    if (owned_)
      swr_v1_core_string_free(slot_);
  }
  void check(int status) const {
    if (status == -2)
      throw std::bad_alloc();
    if (status == -3)
      std::rethrow_exception(cache_->exception);
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
  const std::string &std_string() const {
    if (!cache_) {
      auto view = swr_v1_core_string_view(slot_);
      cache_.reset(new Cache(view.data, view.length));
      swr_v1_core_string_cache(slot_, const_cast<RustStringView *>(this),
                               update);
    }
    return cache_->value;
  }
  operator const std::string &() const { return std_string(); }
};
} // namespace swoole
