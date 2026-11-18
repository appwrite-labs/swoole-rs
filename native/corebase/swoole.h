/* SPDX-License-Identifier: Apache-2.0 */
/* The core declarations native/ and the test fixtures compile against: macros, forward
 * declarations, the IPC records and SwooleG's layout, which Rust holds (crates/corebase), keeping
 * only what a C++ file uses. Was include/swoole.h. */
#pragma once

#ifdef HAVE_CONFIG_H
#include "config.h"
#elif defined(ENABLE_PHP_SWOOLE)
#include "php_config.h"
#endif

#ifdef __cplusplus
#define SW_EXTERN_C_BEGIN extern "C" {
#define SW_EXTERN_C_END }
#else
#define SW_EXTERN_C_BEGIN
#define SW_EXTERN_C_END
#endif

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

/*--- C standard library ---*/
#include <cassert>
#include <cctype>
#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <climits>
#include <unistd.h>
#include <pthread.h>
#include <inttypes.h>

#include <sys/uio.h>

#include <string>
#include <memory>
#include <functional>
#include <type_traits>

typedef unsigned long ulong_t;

#if defined(__GNUC__)
#if __GNUC__ >= 3
#define sw_inline inline __attribute__((always_inline))
#else
#define sw_inline inline
#endif
#elif defined(_MSC_VER)
#define sw_inline __forceinline
#else
#define sw_inline inline
#endif

#if defined(__GNUC__) && __GNUC__ >= 4
#define SW_API __attribute__((visibility("default")))
#else
#define SW_API
#endif

#if !defined(__GNUC__) || __GNUC__ < 3
#define __builtin_expect(x, expected_value) (x)
#endif

#define sw_unlikely(x) __builtin_expect(!!(x), 0)

/*----------------------------------------------------------------------------*/

#include "swoole_rs_corebase_constants.h"
#include "swoole_version.h"
#include "swoole_log.h"
#include "swoole_atomic.h"
#include "swoole_rs_error.h"

#ifndef MAX
#define MAX(A, B) ((A) > (B) ? (A) : (B))
#endif
#ifndef MIN
#define MIN(A, B) ((A) < (B) ? (A) : (B))
#endif
#define SW_LOOP_N(n) for (std::remove_reference_t<decltype(n)> i = 0; i < n; i++)

#ifdef SW_THREAD
#define SW_THREAD_LOCAL thread_local
#else
#define SW_THREAD_LOCAL
#endif

/*-------------------------------Declare Struct--------------------------------*/
namespace swoole {
class Reactor;
class String;
class Timer;
struct Event;
class Pipe;
class UnixSocket;
class MessageBus;
namespace network {
struct Socket;
struct Address;
}  // namespace network
class AsyncThreads;
struct Protocol;
struct DataHead;
typedef std::function<void(void *)> Callback;
}  // namespace swoole

/*----------------------------------String-------------------------------------*/

#define SW_STRL(s) s, sizeof(s) - 1

/** always return less than size, zero termination  */
SW_EXTERN_C_BEGIN
size_t sw_snprintf(char *buf, size_t size, const char *format, ...) __attribute__((format(printf, 3, 4)));
size_t sw_vsnprintf(char *buf, size_t size, const char *format, va_list args);
SW_EXTERN_C_END

//------------------------------Base--------------------------------
#ifndef uchar
typedef unsigned char uchar;
#endif

// sw_error: this thread's SW_ERROR_MSG_SIZE-byte last-error buffer, owned by crates/error.
extern "C" char *swr_v1_error_buffer(void);
#define sw_error (swr_v1_error_buffer())


pid_t swoole_fork(int flags);

namespace swoole {
typedef long SessionId;
typedef long TaskId;
typedef uint32_t WorkerId;
typedef swSocketType SocketType;
typedef swFdType FdType;

struct Event {
    int fd;
    int16_t reactor_id;
    FdType type;
    network::Socket *socket;
};

struct DataHead {
    SessionId fd;
    uint64_t msg_id;
    uint32_t len;
    int16_t reactor_id;
    uint8_t type;
    uint8_t flags;
    uint16_t server_fd;
    uint16_t ext_flags;
    uint32_t reserved;
    double time;
};

}  // namespace swoole

#include "swoole_rs_corebase.h"
#include "swoole_rs_corebase_state.h"

namespace swoole {
// SwooleG's layout: Rust defines the symbol (crates/corebase/src/config_state.rs, which
// asserts the same layout).
struct Global {
    Global() = delete;
    Global(const Global &) = delete;
    uint32_t max_sockets;
    double dns_cache_refresh_time;
    Allocator std_allocator;
};
static_assert(offsetof(Global, dns_cache_refresh_time) == 8 && offsetof(Global, std_allocator) == 16 &&
                  sizeof(Global) == 48,
              "crates/corebase/src/config_state.rs's Global");
}  // namespace swoole

extern swoole::Global SwooleG;  // Local Global Variable

static inline void swoole_set_last_error(int error) {
    swr_v1_core_set_last_error(error);
}

static inline void swoole_clear_last_error() {
    swr_v1_core_set_last_error(0);
}

void swoole_clear_last_error_msg();
const char *swoole_get_last_error_msg();
