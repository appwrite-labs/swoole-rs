/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_string.h, owned here: swoole::String's layout, which
 * crates/buffer/src/string.rs mirrors and whose members it defines under their
 * upstream symbols. Upstream's file is deleted, so `#include "swoole_string.h"`
 * finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole.h"

#include <cstddef>
#include <string>
#include <type_traits>

#define SW_STRINGL(s) s->str, s->length

namespace swoole {
class String {
  public:
    size_t length;
    size_t size;
    off_t offset;
    char *str;
    const Allocator *allocator;

    String() {
        length = size = offset = 0;
        str = nullptr;
        allocator = nullptr;
    }

    explicit String(size_t _size, const Allocator *_allocator = nullptr);
    String(const String &src);
    String(String &&src) noexcept;
    String &operator=(const String &src) = delete;
    String &operator=(String &&src) = delete;
    ~String();

    char *value() const {
        return str;
    }

    size_t get_length() const {
        return length;
    }

    size_t capacity() const {
        return size;
    }

    std::string to_std_string() const {
        return {str, length};
    }

    void clear() {
        length = 0;
        offset = 0;
    }

    void grow(size_t incr_value);
    String substr(size_t offset, size_t len) const;
    void extend();
    void extend(size_t new_size);
    void reserve(size_t new_size);
    char *release();
    void repeat(const char *data, size_t len, size_t n);
    void append(const char *append_str, size_t length);
    void append(const std::string &append_str) {
        append(append_str.c_str(), append_str.length());
    }
    void append(int value);
    bool append_random_bytes(size_t length, bool base64 = false);
    void write(off_t _offset, const String &write_str);
    void set_null_terminated();

    void print(bool print_value = true) const;

    enum FormatFlag {
        FORMAT_APPEND = 1 << 0,
    };

    template <typename... Args>
    size_t format_impl(int flags, const char *format, Args... args) {
        size_t _size = sw_snprintf(nullptr, 0, format, args...);
        if (_size == 0) {
            return 0;
        }
        ++_size;  // the NUL
        size_t n;
        if (flags & FORMAT_APPEND) {
            if (_size > size - length) {
                reserve(length + _size);
            }
            n = sw_snprintf(str + length, size - length, format, args...);
            length += n;
        } else {
            if (_size > size) {
                reserve(_size);
            }
            n = sw_snprintf(str, size, format, args...);
            length = n;
        }
        return n;
    }

    template <typename... Args>
    size_t format(const char *format, Args... args) {
        return format_impl(0, format, args...);
    }

    char *pop(size_t init_size);
    void reduce(off_t offset);
};

static_assert(std::is_standard_layout<String>::value && sizeof(String) == 40 && offsetof(String, offset) == 16 &&
                  offsetof(String, str) == 24 && offsetof(String, allocator) == 32 && sizeof(off_t) == 8,
              "swoole::String matches crates/buffer/src/string.rs");

inline String *make_string(size_t size, const Allocator *allocator = nullptr) {
    return new String(size, allocator);
}
}  // namespace swoole
