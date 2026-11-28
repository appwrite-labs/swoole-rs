// GoogleTest adapter only; original buffer.cpp and all its assertions are intact.
#pragma once
#include "swoole.h"
#include "swoole_string.h"
#include <stdexcept>
#include <string>
#define TEST(group, name) void original_##group##_##name()
#define ASSERT_EQ(left, right) do { if (!((left) == (right))) throw std::runtime_error("Original ASSERT_EQ failed: " #left " == " #right); } while (false)
