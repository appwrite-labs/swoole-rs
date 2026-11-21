// Stand-in for upstream include/swoole.h: only what src/core/crc32.cc uses.
#pragma once
#include <cstddef>
#include <cstdint>

#define sw_unlikely(x) __builtin_expect(!!(x), 0)
