// Assertion adapter only. The upstream test source is compiled byte-for-byte.
#pragma once
#include "swoole.h"
#include "swoole_string.h"
#include <cstring>
#include <stdexcept>
#include <utility>
#include <vector>
#define TEST_STR "hello world, hello swoole\n"
#define TEST_STR2 "I am Rango\n"
using StringTest = std::pair<const char *, void (*)()>;
inline std::vector<StringTest> &string_tests() { static std::vector<StringTest> tests; return tests; }
struct StringTestRegistration {
    StringTestRegistration(const char *name, void (*test)()) { string_tests().emplace_back(name, test); }
};
#define TEST(group, name) static void original_##group##_##name(); static StringTestRegistration registration_##group##_##name(#group "." #name, original_##group##_##name); static void original_##group##_##name()
#define ASSERT_TRUE(value) do { if (!(value)) throw std::runtime_error("Original assertion failed: " #value); } while (false)
#define ASSERT_FALSE(value) ASSERT_TRUE(!(value))
#define ASSERT_EQ(a, b) ASSERT_TRUE((a) == (b))
#define ASSERT_NE(a, b) ASSERT_TRUE((a) != (b))
#define EXPECT_EQ(a, b) ASSERT_EQ(a, b)
#define EXPECT_MEMEQ(a, b, n) ASSERT_EQ(std::memcmp(a, b, n), 0)
#define ASSERT_MEMEQ(a, b, n) EXPECT_MEMEQ(a, b, n)
#define EXPECT_STREQ(a, b) ASSERT_EQ(std::strcmp(a, b), 0)
