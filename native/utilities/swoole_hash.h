/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_hash.h, owned here. Rust defines all four under their upstream
 * symbols (crates/utilities); core-tests/src/core/hash.cpp declares them
 * through this header. */
#pragma once

#include <cstddef>
#include <cstdint>

uint64_t swoole_hash_jenkins(const char *key, size_t keylen);
uint64_t swoole_hash_php(const char *key, size_t len);
uint64_t swoole_hash_austin(const char *key, size_t keylen);
uint32_t swoole_crc32(const char *data, size_t size);
