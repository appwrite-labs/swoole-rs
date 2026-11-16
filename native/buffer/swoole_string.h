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

    String substr(size_t offset, size_t len) const;
    void extend();
    void extend(size_t new_size);
    void reserve(size_t new_size);
    char *release();
    void append(int value);
    bool append_random_bytes(size_t length, bool base64 = false);
    void write(off_t _offset, const String &write_str);

    void print(bool print_value = true) const;

    char *pop(size_t init_size);
    void reduce(off_t offset);
};

static_assert(std::is_standard_layout<String>::value && sizeof(String) == 40 && offsetof(String, offset) == 16 &&
                  offsetof(String, str) == 24 && offsetof(String, allocator) == 32 && sizeof(off_t) == 8,
              "swoole::String matches crates/buffer/src/string.rs");
}  // namespace swoole
