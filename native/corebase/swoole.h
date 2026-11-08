/* SPDX-License-Identifier: Apache-2.0 */
/* The core declarations native/ and the test fixtures compile against: macros, forward
 * declarations, the IPC records and SwooleG, whose values Rust holds (crates/corebase/src/global.rs),
 * keeping only what a C++ file uses. Was include/swoole.h. */
#pragma once

#include "swoole_rs_utilities.h"

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
#include <sys/utsname.h>

#include <string>
#include <memory>
#include <list>
#include <functional>
#include <type_traits>
#include <mutex>

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

#define sw_likely(x) __builtin_expect(!!(x), 1)
#define sw_unlikely(x) __builtin_expect(!!(x), 0)

/*----------------------------------------------------------------------------*/

#include "swoole_rs_corebase_constants.h"
#include "swoole_version.h"
#include "swoole_log.h"
#include "swoole_atomic.h"
#include "swoole_rs_error.h"

// A log emitted on behalf of an upstream function, for example from a Rust dispatch callback,
// names that function as upstream's __SW_FUNC__ did:
// swoole_log_as("Server::timer_callback", swoole_warning("...")).
static constexpr const char *sw_log_origin = nullptr;
#undef __SW_FUNC__
#define __SW_FUNC__ (sw_log_origin ? sw_log_origin : swoole::Logger::get_pretty_name(__PRETTY_FUNCTION__).c_str())
#define swoole_log_as(origin, ...)                                                                                     \
    do {                                                                                                               \
        const char *const sw_log_origin = origin;                                                                      \
        (void) sw_log_origin;                                                                                          \
        __VA_ARGS__;                                                                                                   \
    } while (0)

#define SW_MIN(A, B) ((A) < (B) ? (A) : (B))
#ifndef MAX
#define MAX(A, B) ((A) > (B) ? (A) : (B))
#endif
#ifndef MIN
#define MIN(A, B) SW_MIN(A, B)
#endif
#define SW_LOOP_N(n) for (std::remove_reference_t<decltype(n)> i = 0; i < n; i++)

#ifdef SW_DEBUG
#define SW_ASSERT(e) assert(e)
#else
#define SW_ASSERT(e)
#endif

#ifdef SW_THREAD
#define SW_THREAD_LOCAL thread_local
#else
#define SW_THREAD_LOCAL
#endif

extern std::mutex sw_thread_lock;

/*-----------------------------------Memory------------------------------------*/
void *sw_malloc(size_t size);
void sw_free(void *ptr);

#define SW_MEM_ALIGNED_SIZE_EX(size, alignment) (((size) + ((alignment) -1LL)) & ~((alignment) -1LL))

/*-------------------------------Declare Struct--------------------------------*/
namespace swoole {
class MemoryPool;
class Reactor;
class String;
class Timer;
struct TimerNode;
struct Event;
class Pipe;
class UnixSocket;
class MessageBus;
class Server;
namespace network {
struct Socket;
struct Address;
}  // namespace network
class AsyncThreads;
struct Protocol;
struct EventData;
struct DataHead;
typedef int (*ReactorHandler)(Reactor *reactor, Event *event);
typedef std::function<void(void *)> Callback;
typedef void (*TimerCallback)(Timer *, TimerNode *);
}  // namespace swoole

/*----------------------------------String-------------------------------------*/

#define SW_STRL(s) s, sizeof(s) - 1

/** always return less than size, zero termination  */
SW_EXTERN_C_BEGIN
size_t sw_snprintf(char *buf, size_t size, const char *format, ...) __attribute__((format(printf, 3, 4)));
size_t sw_vsnprintf(char *buf, size_t size, const char *format, va_list args);
SW_EXTERN_C_END

static inline void sw_memset_zero(void *s, size_t n) {
    memset(s, 0, n);
}

static inline unsigned int swoole_str_istarts_with(const char *pstr, size_t plen, const char *sstr, size_t slen) {
    return swr_v1_utils_compare(reinterpret_cast<const uint8_t *>(pstr), plen,
                                reinterpret_cast<const uint8_t *>(sstr), slen, 3);
}

static inline const char *swoole_strnstr(const char *haystack,
                                         uint32_t haystack_length,
                                         const char *needle,
                                         uint32_t needle_length) {
    assert(needle_length > 0);
    return swr_v2_utils_search(haystack, haystack_length, needle, needle_length, 0).pointer;
}

static inline ssize_t swoole_strnpos(const char *haystack,
                                     uint32_t haystack_length,
                                     const char *needle,
                                     uint32_t needle_length) {
    assert(needle_length > 0);

    return swr_v2_utils_search(haystack, haystack_length, needle, needle_length, 0).offset;
}


//------------------------------Base--------------------------------
#ifndef uchar
typedef unsigned char uchar;
#endif

// sw_error: this thread's SW_ERROR_MSG_SIZE-byte last-error buffer, owned by crates/error.
extern "C" char *swr_v1_error_buffer(void);
#define sw_error (swr_v1_error_buffer())

// tests/modules/utilities/storage.py declares these from here for the pinned hash tests.
uint64_t swoole_random_int();
size_t swoole_random_bytes(char *buf, size_t size);

//----------------------Tool Function---------------------
uint32_t swoole_common_multiple(uint32_t u, uint32_t v);
uint32_t swoole_common_divisor(uint32_t u, uint32_t v);

int swoole_itoa(char *buf, long value);

int swoole_rand();
int swoole_rand(int min, int max);

int swoole_version_compare(const char *version1, const char *version2);

bool swoole_get_env(const char *name, int *value);

void swoole_init();
void swoole_exit(int _status);
pid_t swoole_fork(int flags);
pid_t swoole_fork_exec(const std::function<void()> &child_fn);
pid_t swoole_waitpid(pid_t _pid, int *_stat_loc, int _options);
void swoole_thread_init(bool main_thread = false);
void swoole_thread_clean(bool main_thread = false);
extern "C" void swr_v1_os_set_isolation(const char *group, const char *user, const char *root);
inline void swoole_set_isolation(const std::string &group_, const std::string &user_, const std::string &chroot_) {
    swr_v1_os_set_isolation(group_.c_str(), user_.c_str(), chroot_.c_str());
}
bool swoole_set_task_tmpdir(const std::string &dir);
std::string swoole_get_task_tmpdir();

namespace swoole {
typedef long SessionId;
typedef long TaskId;
typedef uint8_t ReactorId;
typedef uint32_t WorkerId;
typedef swEventType EventType;
typedef swSocketType SocketType;
typedef swTimeoutType TimeoutType;
typedef swFdType FdType;
typedef swReturnCode ReturnCode;

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

struct EventData {
    DataHead info;
    char data[SW_IPC_BUFFER_SIZE];

    uint32_t size() const {
        return sizeof(info) + len();
    }

    uint32_t len() const {
        return info.len;
    }
};

}  // namespace swoole

#include "swoole_rs_corebase.h"
#include "swoole_rs_corebase_state.h"
#include "swoole_rs_corebase_string_view.h"

namespace swoole {
// A SwooleG member whose value Rust holds (crates/corebase/src/global.rs): reads
// and writes go through its accessor pair, so `SwooleG.field` code compiles as
// before. It is never copied, so it can't be passed through `...` by mistake.
template <typename T, typename R, R (*get)(), void (*set)(R)>
struct RustField {
    RustField() = default;
    RustField(const RustField &) = delete;
    // `SwooleG = {}` leaves the record to swr_v1_core_reset.
    RustField &operator=(const RustField &) {
        return *this;
    }
    RustField &operator=(T value) {
        set((R) value);
        return *this;
    }
    operator T() const {
        return (T) get();
    }
    T operator->() const {
        return (T) get();
    }
    RustField &operator+=(T delta) {
        return *this = (T) (*this + delta);
    }
    T operator++(int) {
        T old = *this;
        *this = (T) (old + 1);
        return old;
    }
    T operator--(int) {
        T old = *this;
        *this = (T) (old - 1);
        return old;
    }
};

#define SW_RUST_FIELD(type, name)                                                                                     \
    RustField<type, decltype(swr_v1_core_##name()), swr_v1_core_##name, swr_v1_core_set_##name> name

struct Global : private CoreConfigOwner {
    Global() = default;
    Global(const Global &) = delete;
    swr_v1_core_config *config_handle() const { return config_.owner; }
    Global &operator=(const Global &source) {
        auto copied = swr_v1_core_config_copy_values(config_handle(), source.config_handle());
        RustStringView *views[] = {&task_tmpfile, &dns_resolvconf_path};
        if (copied.status) views[copied.index]->check(copied.status);
        swr_v1_core_name_resolver_clear();  // the only source is `{}`
        return *this;
    }
    SW_RUST_FIELD(bool, running);
    SW_RUST_FIELD(bool, enable_signalfd);
    SW_RUST_FIELD(bool, enable_coroutine);

    SW_RUST_FIELD(uint32_t, signal_listener_num);
    SW_RUST_FIELD(uint32_t, signal_async_listener_num);

    //-----------------------[System]--------------------------
    SW_RUST_FIELD(uint32_t, pagesize);
    uint32_t &max_sockets = config_.fields->max_sockets;
    //-----------------------[Memory]--------------------------
    Allocator &std_allocator = config_.fields->std_allocator;
    RustStringView task_tmpfile{config_.strings[0]};
    //------------------[Single Instance]----------------------
    SW_RUST_FIELD(Server *, server);
    //-----------------------[DNS]-----------------------------
    double &dns_cache_refresh_time = config_.fields->dns_cache_refresh_time;
    RustStringView dns_resolvconf_path{config_.strings[1]};
};

double microtime();
}  // namespace swoole

extern swoole::Global SwooleG;                      // Local Global Variable
extern thread_local swoole::ThreadGlobal SwooleTG;  // Thread Global Variable

#define SW_CPU_NUM (swr_v1_core_cpu_num())

static inline void swoole_set_last_error(int error) {
    swr_v1_core_set_last_error(error);
}

static inline int swoole_get_last_error() {
    return swr_v1_core_last_error();
}

static inline void swoole_clear_last_error() {
    swr_v1_core_set_last_error(0);
}

void swoole_clear_last_error_msg();
const char *swoole_get_last_error_msg();

static inline int swoole_get_thread_id() {
    return SwooleTG.id;
}

static inline int swoole_get_thread_type() {
    return SwooleTG.type;
}

static inline void swoole_set_thread_id(uint16_t id) {
    SwooleTG.id = id;
}

static inline void swoole_set_thread_type(uint8_t type) {
    SwooleTG.type = type;
}

static inline uint32_t swoole_pagesize() {
    return SwooleG.pagesize;
}

static sw_inline swoole::String *sw_tg_buffer() {
    return SwooleTG.buffer_stack;
}

static sw_inline swoole::MemoryPool *sw_mem_pool() {
    return static_cast<swoole::MemoryPool *>(swr_v1_core_memory_pool());
}

static sw_inline swoole::Reactor *sw_reactor() {
    return SwooleTG.reactor;
}

static sw_inline swoole::Timer *sw_timer() {
    return SwooleTG.timer;
}
