// What the Rust cases in src/ can't express, compiled into system.py's lib-swoole by
// tests/modules/hooks/system.py (--fixture): swoole_event_defer and the timer, which have C++
// linkage. The witnesses of the resolver record's C++ views (formerly
// tests/modules/hooks/dns_request_owner.cpp) went with those views; what they checked of
// the record is tests/modules/corebase/resolver.rs.
#include "swoole_coroutine.h"
#include "swoole_coroutine_system.h"
#include "swoole_rs_coroutine_socket.h"
#include "swoole_async.h"
#include "swoole_rs_process_pool.h"

#include <arpa/inet.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <unistd.h>

using namespace swoole;

// A failed assertion ends the case's process, which the runner reports as failed.
#define CHECK(c)                                                                                                       \
    do {                                                                                                               \
        if (!(c)) {                                                                                                    \
            fprintf(stderr, "%s:%d: assertion failed: %s\n", __FILE__, __LINE__, #c);                                  \
            _exit(1);                                                                                                  \
        }                                                                                                              \
    } while (0)
#define ASSERT_TRUE(c) CHECK(c)
#define ASSERT_FALSE(c) CHECK(!(c))
#define ASSERT_EQ(a, b) CHECK((a) == (b))
#define ASSERT_NE(a, b) CHECK((a) != (b))
#define ASSERT_GT(a, b) CHECK((a) > (b))
#define EXPECT_THROW(statement, type)                                                                                  \
    do {                                                                                                               \
        bool thrown = false;                                                                                           \
        try {                                                                                                          \
            statement;                                                                                                 \
        } catch (const type &) {                                                                                       \
            thrown = true;                                                                                             \
        }                                                                                                              \
        CHECK(thrown);                                                                                                 \
    } while (0)

// The fault oracle: between begin(n) and end(), this thread's nth operator new throws.
namespace allocation {
thread_local bool active = false;
thread_local size_t count = 0, fail = 0;
void begin(size_t nth) {
    count = 0;
    fail = nth;
    active = true;
}
void end() {
    active = false;
}
}  // namespace allocation
void *operator new(size_t size) {
    if (allocation::active && ++allocation::count == allocation::fail) {
        throw std::bad_alloc();
    }
    if (void *p = std::malloc(size ? size : 1)) {
        return p;
    }
    throw std::bad_alloc();
}
void operator delete(void *p) noexcept {
    std::free(p);
}
void *operator new[](size_t n) {
    return ::operator new(n);
}
void operator delete[](void *p) noexcept {
    ::operator delete(p);
}
void operator delete(void *p, size_t) noexcept {
    ::operator delete(p);
}
void operator delete[](void *p, size_t) noexcept {
    ::operator delete(p);
}
// Keep nothrow allocations in the same malloc/free domain as the fault oracle.
void *operator new(size_t n, const std::nothrow_t &) noexcept {
    try {
        return ::operator new(n);
    } catch (...) {
        return nullptr;
    }
}
void *operator new[](size_t n, const std::nothrow_t &) noexcept {
    try {
        return ::operator new[](n);
    } catch (...) {
        return nullptr;
    }
}
void operator delete(void *p, const std::nothrow_t &) noexcept {
    ::operator delete(p);
}
void operator delete[](void *p, const std::nothrow_t &) noexcept {
    ::operator delete[](p);
}

extern "C" {
// `swoole_event_defer(fn, data)`.
void swr_case_event_defer(void (*fn)(void *), void *data) {
    swoole_event_defer(fn, data);
}

// SwooleTG.timer, a C++ thread_local (timer.rs).
Timer *swr_case_timer_exchange(Timer *timer) {
    Timer *previous = SwooleTG.timer;
    SwooleTG.timer = timer;
    return previous;
}

// os/process_pool.cpp (process_pool.rs): the record sizes its mirrors check, and what its
// TESTs reach that isn't an exported symbol: SwooleWG and sw_worker().
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
extern const size_t swr_case_process_layout[] = {sizeof(ProcessPool),
                                                 offsetof(ProcessPool, main_loop),
                                                 offsetof(ProcessPool, ptr),
                                                 sizeof(Worker),
                                                 offsetof(Worker, start_time),
                                                 sizeof(EventData)};
#pragma GCC diagnostic pop
// `SwooleWG`: crates/process's WorkerGlobalRecord, up to the fields read here.
struct CaseWorkerGlobal {
    uint32_t id;
    uint8_t type;
    pid_t pid;
    bool shutdown;
    bool running;
    uint32_t max_request;
    Worker *worker;
};
static_assert(offsetof(CaseWorkerGlobal, max_request) == 16, "WorkerGlobalRecord::max_request");
static_assert(offsetof(CaseWorkerGlobal, worker) == 24, "WorkerGlobalRecord::worker");
extern "C" CaseWorkerGlobal *swr_v1_worker_globals();

uint32_t swr_case_worker_max_request() {
    return swr_v1_worker_globals()->max_request;
}
Worker *swr_case_current_worker() {
    return swr_v1_worker_globals()->worker;
}

}
